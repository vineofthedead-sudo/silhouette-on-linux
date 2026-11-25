//! A virtual cutter: answers status and firmware queries like a real machine and records the
//! job, which [`ss_gpgl::simulate`] turns back into tool paths.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ss_gpgl::{ESC, ETX, Simulation};

use crate::{DeviceError, Transport};

/// What a cutter answers when it found the registration marks.
pub(crate) const REGISTRATION_OK: &[u8] = b"    0\x03";

/// Picks queries out of an outgoing byte stream, which may arrive split at any point.
#[derive(Debug, Default)]
pub(crate) struct QueryScanner {
    after_esc: bool,
    command: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Query {
    Status,
    ToolSetup,
    Firmware,
    Initialize,
    /// A registration mark search; unlike the others it is also part of the job.
    Registration,
}

impl QueryScanner {
    /// Feeds bytes; returns the queries seen and the bytes that belong to job commands.
    pub(crate) fn feed(&mut self, data: &[u8]) -> (Vec<Query>, Vec<u8>) {
        let mut queries = Vec::new();
        let mut job = Vec::new();
        for &b in data {
            if self.after_esc {
                self.after_esc = false;
                match b {
                    0x05 => queries.push(Query::Status),
                    0x15 => queries.push(Query::ToolSetup),
                    0x04 => queries.push(Query::Initialize),
                    _ => {}
                }
            } else if b == ESC && self.command.is_empty() {
                self.after_esc = true;
            } else {
                self.command.push(b);
                if b == ETX {
                    if self.command == b"FG\x03" {
                        queries.push(Query::Firmware);
                    } else {
                        if self.command.starts_with(b"TB123,") || self.command.starts_with(b"TB23,") {
                            queries.push(Query::Registration);
                        }
                        job.extend_from_slice(&self.command);
                    }
                    self.command.clear();
                }
            }
        }
        (queries, job)
    }
}

#[derive(Debug, Default)]
struct State {
    scanner: QueryScanner,
    job: Vec<u8>,
    replies: VecDeque<u8>,
    unloaded: bool,
    busy_polls: u32,
    accept_limit: Option<usize>,
    stalled: bool,
    initialized: u32,
    /// `None` answers registration with success; `Some(None)` never answers.
    registration_reply: Option<Option<Vec<u8>>>,
}

/// Shared view of a [`VirtualCutter`] for tests and previews.
#[derive(Debug, Clone, Default)]
pub struct VirtualHandle(Arc<Mutex<State>>);

impl VirtualHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Job command bytes received so far (queries removed).
    pub fn received(&self) -> Vec<u8> {
        self.lock().job.clone()
    }

    pub fn simulation(&self) -> Simulation {
        ss_gpgl::simulate(&self.received())
    }

    /// Times `ESC EOT` was received.
    pub fn initialized(&self) -> u32 {
        self.lock().initialized
    }

    /// Reports "no media loaded" until cleared.
    pub fn set_unloaded(&self, unloaded: bool) {
        self.lock().unloaded = unloaded;
    }

    /// Reports "moving" for the next `n` status polls.
    pub fn set_busy_polls(&self, n: u32) {
        self.lock().busy_polls = n;
    }

    /// Accepts at most `n` bytes per write call.
    pub fn set_accept_limit(&self, n: Option<usize>) {
        self.lock().accept_limit = n;
    }

    /// How the next registration searches are answered: `Some(bytes)` sends that reply, `None`
    /// never answers (marks not found). The default reports success.
    pub fn set_registration_reply(&self, reply: Option<Vec<u8>>) {
        self.lock().registration_reply = Some(reply);
    }

    /// Accepts no job data at all, like a cutter whose buffer stays full.
    pub fn set_stalled(&self, stalled: bool) {
        self.lock().stalled = stalled;
    }
}

pub struct VirtualCutter {
    handle: VirtualHandle,
}

impl VirtualCutter {
    pub fn new() -> (Self, VirtualHandle) {
        let handle = VirtualHandle::default();
        (Self { handle: handle.clone() }, handle)
    }
}

impl Transport for VirtualCutter {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, DeviceError> {
        let mut s = self.handle.lock();
        // Control sequences always get through; only job data is subject to stalls and limits.
        let is_control = data.first() == Some(&ESC) && data.len() == 2 || data == b"FG\x03";
        if s.stalled && !is_control {
            drop(s);
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
            return Err(DeviceError::Timeout);
        }
        let n = if is_control { data.len() } else { s.accept_limit.map_or(data.len(), |l| l.min(data.len())) };
        let (queries, job) = s.scanner.feed(&data[..n]);
        s.job.extend(job);
        for q in queries {
            let reply: &[u8] = match q {
                Query::Status if s.unloaded => b"2\x03",
                Query::Status if s.busy_polls > 0 => {
                    s.busy_polls -= 1;
                    b"1\x03"
                }
                Query::Status => b"0\x03",
                Query::Firmware => b" VIRTUAL 1.0\x03",
                Query::ToolSetup => b"none\x03",
                Query::Initialize => {
                    s.initialized += 1;
                    continue;
                }
                Query::Registration => match s.registration_reply.clone() {
                    None => {
                        s.replies.extend(REGISTRATION_OK);
                        continue;
                    }
                    Some(Some(r)) => {
                        s.replies.extend(r);
                        continue;
                    }
                    Some(None) => continue,
                },
            };
            s.replies.extend(reply);
        }
        Ok(n)
    }

    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, DeviceError> {
        let mut s = self.handle.lock();
        if s.replies.is_empty() {
            drop(s);
            std::thread::sleep(timeout.min(Duration::from_millis(2)));
            return Ok(Vec::new());
        }
        Ok(s.replies.drain(..).collect())
    }

    fn description(&self) -> String {
        "Virtual cutter".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_handles_split_input() {
        let mut sc = QueryScanner::default();
        let stream = b"\x1b\x04FN0\x03\x1b\x05F\x03FG\x03M1,2\x03TB123,1,2,3,4\x03";
        let mut queries = Vec::new();
        let mut job = Vec::new();
        for chunk in stream.chunks(3) {
            let (q, j) = sc.feed(chunk);
            queries.extend(q);
            job.extend(j);
        }
        assert_eq!(queries, vec![Query::Initialize, Query::Status, Query::Firmware, Query::Registration]);
        assert_eq!(job, b"FN0\x03F\x03M1,2\x03TB123,1,2,3,4\x03");
    }
}

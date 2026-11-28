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
    Wakeup,
    ToolSetup1,
    ToolSetup2,
    Firmware,
    Ti,
    To,
    Tb71,
    Fa,
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
                    0x11 => queries.push(Query::Wakeup),
                    0x15 => queries.push(Query::ToolSetup1),
                    0x16 => queries.push(Query::ToolSetup2),
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
                    } else if self.command == b"TI\x03" {
                        queries.push(Query::Ti);
                    } else if self.command == b"TO\x03" {
                        queries.push(Query::To);
                    } else if self.command == b"TB71\x03" {
                        queries.push(Query::Tb71);
                    } else if self.command == b"FA\x03" {
                        queries.push(Query::Fa);
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
    firmware: Option<String>,
    tool1_reply: Option<Vec<u8>>,
    tool2_reply: Option<Vec<u8>>,
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

    /// Sets the firmware string returned by `FG` and `ESC 0x11` (without trailing ETX).
    pub fn set_firmware(&self, fw: impl Into<String>) {
        self.lock().firmware = Some(fw.into());
    }

    /// Sets the raw reply (including ETX) for `ESC 0x15` (holder 1) or `ESC 0x16` (holder 2).
    pub fn set_tool_reply(&self, holder: u8, reply: Vec<u8>) {
        let mut s = self.lock();
        if holder == 2 {
            s.tool2_reply = Some(reply);
        } else {
            s.tool1_reply = Some(reply);
        }
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
        let is_control = (data.first() == Some(&ESC) && data.len() == 2)
            || matches!(data, b"FG\x03" | b"TI\x03" | b"TO\x03" | b"TB71\x03" | b"FA\x03");
        if s.stalled && !is_control {
            drop(s);
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
            return Err(DeviceError::Timeout);
        }
        let n = if is_control { data.len() } else { s.accept_limit.map_or(data.len(), |l| l.min(data.len())) };
        let (queries, job) = s.scanner.feed(&data[..n]);
        s.job.extend(job);
        for q in queries {
            match q {
                Query::Status if s.unloaded => s.replies.extend(b"2\x03"),
                Query::Status if s.busy_polls > 0 => {
                    s.busy_polls -= 1;
                    s.replies.extend(b"1\x03");
                }
                Query::Status => s.replies.extend(b"0\x03"),
                Query::Firmware | Query::Wakeup => {
                    let fw = s.firmware.clone().unwrap_or_else(|| " VIRTUAL 1.0".into());
                    s.replies.extend(fw.as_bytes());
                    s.replies.push_back(ETX);
                }
                Query::ToolSetup1 => {
                    let reply = s.tool1_reply.clone().unwrap_or_else(|| b" 2, 0\x03".to_vec());
                    s.replies.extend(reply);
                }
                Query::ToolSetup2 => {
                    let reply = s.tool2_reply.clone().unwrap_or_else(|| b"   0\x03".to_vec());
                    s.replies.extend(reply);
                }
                Query::Ti => s.replies.push_back(ETX),
                Query::To => s.replies.extend(b"0\x03"),
                Query::Tb71 | Query::Fa => s.replies.extend(b"    0,    0\x03"),
                Query::Initialize => {
                    s.initialized += 1;
                }
                Query::Registration => match s.registration_reply.clone() {
                    None => s.replies.extend(REGISTRATION_OK),
                    Some(Some(r)) => s.replies.extend(r),
                    Some(None) => {}
                },
            }
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

    #[test]
    fn cutter_wakeup_and_tool_queries_on_virtual_cutter() {
        let (vc, h) = VirtualCutter::new();
        h.set_firmware("CAMEO 4 V1.20    ");
        h.set_tool_reply(1, b" 2, 0\x03".to_vec());
        h.set_tool_reply(2, b"   0\x03".to_vec());

        let mut cutter = crate::Cutter::new(vc);
        let fw = cutter.wakeup().unwrap();
        assert_eq!(fw.as_deref(), Some("CAMEO 4 V1.20"));
        assert_eq!(cutter.query_tool(1).unwrap(), ss_gpgl::DetectedTool::AutoBlade);
        assert_eq!(cutter.query_tool(2).unwrap(), ss_gpgl::DetectedTool::Empty);

        h.set_tool_reply(1, b" 1, 0\x03".to_vec());
        assert_eq!(cutter.query_tool(1).unwrap(), ss_gpgl::DetectedTool::ManualBlade);
    }
}

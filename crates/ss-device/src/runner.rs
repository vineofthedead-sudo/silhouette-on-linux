//! Runs a cut job on a background thread with progress events and cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use ss_gpgl::Status;

use crate::{Cutter, DeviceError, Transport};

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Connected { device: String, firmware: Option<String> },
    /// The cutter reports no media; load the mat to continue.
    WaitingForMedia,
    Sending { sent: usize, total: usize },
    /// The cutter is scanning for the registration marks.
    Registering,
    /// The marks were found; cutting continues relative to them.
    Registered,
    /// All data is sent; the cutter is still working through its buffer.
    Finishing,
    Finished,
    Cancelled,
    Failed(String),
}

impl JobEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished | Self::Cancelled | Self::Failed(_))
    }
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// How long to wait for media to be loaded and the cutter to become ready.
    pub ready_timeout: Duration,
    /// Give up when the cutter accepts no data for this long.
    pub stall_timeout: Duration,
    /// Wait until the cutter reports ready after the last byte.
    pub wait_for_finish: bool,
    pub finish_timeout: Duration,
    pub poll_interval: Duration,
    /// How long the registration mark search may take.
    pub registration_timeout: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(300),
            stall_timeout: Duration::from_secs(120),
            wait_for_finish: true,
            finish_timeout: Duration::from_secs(4 * 3600),
            poll_interval: Duration::from_millis(500),
            registration_timeout: Duration::from_secs(40),
        }
    }
}

pub struct JobHandle {
    pub events: Receiver<JobEvent>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl JobHandle {
    /// Stops sending at the next chunk boundary. The cutter finishes the commands it has
    /// already buffered.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Waits for the job to end and returns every event not yet received.
    pub fn wait(mut self) -> Vec<JobEvent> {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.events.try_iter().collect()
    }
}

/// Starts sending `job` (a complete GPGL stream) on its own thread.
pub fn spawn_job<T: Transport + 'static>(transport: T, job: Vec<u8>, opts: RunOptions) -> JobHandle {
    spawn(transport, job, None, opts)
}

/// Like [`spawn_job`] for an encoded job: with registration, sending pauses after the mark
/// search until the cutter reports that it found the marks.
pub fn spawn_cut_job<T: Transport + 'static>(transport: T, job: ss_gpgl::Job, opts: RunOptions) -> JobHandle {
    spawn(transport, job.bytes, job.registration_end, opts)
}

fn spawn<T: Transport + 'static>(transport: T, job: Vec<u8>, registration_end: Option<usize>, opts: RunOptions) -> JobHandle {
    let (tx, rx) = unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let thread = std::thread::Builder::new()
        .name("cut-job".into())
        .spawn(move || {
            let end = match run(transport, &job, registration_end, &opts, &flag, &tx) {
                Ok(()) => JobEvent::Finished,
                Err(DeviceError::Cancelled) => JobEvent::Cancelled,
                Err(e) => JobEvent::Failed(e.to_string()),
            };
            let _ = tx.send(end);
        })
        .expect("spawn job thread");
    JobHandle { events: rx, cancel, thread: Some(thread) }
}

fn run<T: Transport>(
    transport: T,
    job: &[u8],
    registration_end: Option<usize>,
    opts: &RunOptions,
    cancel: &AtomicBool,
    tx: &Sender<JobEvent>,
) -> Result<(), DeviceError> {
    let cancelled = || cancel.load(Ordering::SeqCst);
    let mut cutter = Cutter::new(transport);
    cutter.initialize()?;
    let firmware = cutter.firmware().unwrap_or(None);
    let _ = tx.send(JobEvent::Connected { device: cutter.transport().description(), firmware });

    let mut told_media = false;
    cutter.wait_for(|s| *s == Status::Ready, opts.ready_timeout, opts.poll_interval, &cancelled, &mut |s| {
        if *s == Status::Unloaded && !told_media {
            told_media = true;
            let _ = tx.send(JobEvent::WaitingForMedia);
        }
    })?;

    let total = job.len();
    let split = registration_end.filter(|&e| e <= total).unwrap_or(0);
    let mut last_pct = usize::MAX;
    let mut report = |sent: usize| {
        let pct = sent * 100 / total.max(1);
        if pct != last_pct || sent == total {
            last_pct = pct;
            let _ = tx.send(JobEvent::Sending { sent, total });
        }
    };
    if split > 0 {
        cutter.write_all(&job[..split], opts.stall_timeout, &cancelled, &mut report)?;
        let _ = tx.send(JobEvent::Registering);
        cutter.wait_registration(opts.registration_timeout, &cancelled)?;
        let _ = tx.send(JobEvent::Registered);
    }
    cutter.write_all(&job[split..], opts.stall_timeout, &cancelled, &mut |sent| report(split + sent))?;

    if opts.wait_for_finish {
        let _ = tx.send(JobEvent::Finishing);
        cutter.wait_for(|s| *s == Status::Ready, opts.finish_timeout, opts.poll_interval, &cancelled, &mut |_| {})?;
    }
    Ok(())
}

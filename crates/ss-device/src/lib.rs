//! Cutter transports, the cutter protocol layer and the background job runner.
//!
//! Silhouette cutters take a plain byte stream: GPGL goes out on a bulk OUT endpoint and replies
//! come back on bulk IN. The Windows application writes to the device handle with overlapped
//! `WriteFile`/`ReadFile` and no extra framing (`docs/PHASE1_INVESTIGATION.md` §5.3).

pub mod file;
pub mod models;
pub mod runner;
pub mod usb;
pub mod virtual_cutter;

use std::time::{Duration, Instant};

use ss_gpgl::{DetectedTool, ETX, Status, control};

pub use file::FileTransport;
pub use models::{Model, ProtocolEra, find_model};
pub use runner::{JobEvent, JobHandle, RunOptions, spawn_cut_job, spawn_job};
pub use usb::{UsbDeviceInfo, UsbTransport, enumerate};
pub use virtual_cutter::{VirtualCutter, VirtualHandle};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("USB error: {0}")]
    Usb(rusb::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the cutter did not respond in time")]
    Timeout,
    #[error("no cutter found")]
    NotFound,
    #[error("permission denied opening the cutter: install packaging/70-silhouette-cutters.rules and replug it")]
    PermissionDenied,
    #[error("the cutter was disconnected")]
    Disconnected,
    #[error("the cutter is in use by another program")]
    Busy,
    #[error("unexpected reply from the cutter: {0:?}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
    #[error("registration marks not found: {0}")]
    Registration(String),
}

impl From<rusb::Error> for DeviceError {
    fn from(e: rusb::Error) -> Self {
        match e {
            rusb::Error::Timeout => Self::Timeout,
            rusb::Error::Access => Self::PermissionDenied,
            rusb::Error::NoDevice => Self::Disconnected,
            rusb::Error::NotFound => Self::NotFound,
            rusb::Error::Busy => Self::Busy,
            other => Self::Usb(other),
        }
    }
}

/// A byte pipe to a cutter.
pub trait Transport: Send {
    /// Writes a prefix of `data` and returns how many bytes the device accepted. Returns
    /// [`DeviceError::Timeout`] when it accepts nothing within `timeout` (its buffer is full
    /// while it cuts).
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, DeviceError>;
    /// Returns whatever the device sends within `timeout`; empty when it sends nothing.
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, DeviceError>;
    fn description(&self) -> String;
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, DeviceError> {
        (**self).write(data, timeout)
    }
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, DeviceError> {
        (**self).read(timeout)
    }
    fn description(&self) -> String {
        (**self).description()
    }
}

/// Tool setup query on Cameo 4 class machines (`ESC NAK`).
pub const TOOL_SETUP_QUERY: &[u8] = &[ss_gpgl::ESC, 0x15];

/// Protocol operations on top of a transport.
pub struct Cutter<T: Transport> {
    transport: T,
    /// Per-call I/O timeout.
    pub io_timeout: Duration,
}

impl<T: Transport> Cutter<T> {
    pub fn new(transport: T) -> Self {
        Self { transport, io_timeout: Duration::from_millis(500) }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    /// Sends all of `data`. Gives up when the device accepts nothing for `stall_limit`.
    /// `progress` gets the running byte count after each accepted write.
    pub fn write_all(
        &mut self,
        data: &[u8],
        stall_limit: Duration,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(usize),
    ) -> Result<(), DeviceError> {
        let mut sent = 0;
        let mut last_accept = Instant::now();
        while sent < data.len() {
            if cancelled() {
                return Err(DeviceError::Cancelled);
            }
            match self.transport.write(&data[sent..], self.io_timeout) {
                Ok(n) if n > 0 => {
                    sent += n;
                    last_accept = Instant::now();
                    progress(sent);
                }
                Ok(_) | Err(DeviceError::Timeout) => {
                    if last_accept.elapsed() >= stall_limit {
                        return Err(DeviceError::Timeout);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        self.write_all(data, self.io_timeout * 4, &|| false, &mut |_| {})
    }

    /// Discards replies left over from earlier commands.
    fn drain(&mut self) -> Result<(), DeviceError> {
        for _ in 0..16 {
            if self.transport.read(Duration::from_millis(5))?.is_empty() {
                break;
            }
        }
        Ok(())
    }

    /// Sends a query and returns the reply up to and including ETX.
    pub fn query(&mut self, cmd: &[u8], timeout: Duration) -> Result<Vec<u8>, DeviceError> {
        self.drain()?;
        self.send(cmd)?;
        let deadline = Instant::now() + timeout;
        let mut reply = Vec::new();
        while !reply.contains(&ETX) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(DeviceError::Timeout);
            }
            reply.extend(self.transport.read(left.min(self.io_timeout))?);
        }
        let end = reply.iter().position(|b| *b == ETX).map_or(reply.len(), |i| i + 1);
        reply.truncate(end);
        Ok(reply)
    }

    pub fn initialize(&mut self) -> Result<(), DeviceError> {
        self.send(control::INITIALIZE)
    }

    pub fn status(&mut self) -> Result<Status, DeviceError> {
        Ok(ss_gpgl::parse_status(&self.query(control::STATUS, Duration::from_secs(5))?))
    }

    pub fn firmware(&mut self) -> Result<Option<String>, DeviceError> {
        Ok(ss_gpgl::parse_firmware(&self.query(control::FIRMWARE, Duration::from_secs(5))?))
    }

    /// Wakeup / model identification query (`ESC 0x11`).
    pub fn wakeup(&mut self) -> Result<Option<String>, DeviceError> {
        Ok(ss_gpgl::parse_firmware(&self.query(control::WAKEUP, Duration::from_secs(5))?))
    }

    /// Polls the optical carriage tool sensor (`ESC 0x15` for holder 1, `ESC 0x16` for holder 2).
    pub fn query_tool(&mut self, holder: u8) -> Result<DetectedTool, DeviceError> {
        let cmd = if holder == 2 { control::TOOL_QUERY_2 } else { control::TOOL_QUERY_1 };
        Ok(ss_gpgl::parse_tool_sensor(&self.query(cmd, Duration::from_secs(5))?))
    }

    /// Installed tools on Cameo 4 class machines.
    pub fn tool_setup(&mut self) -> Result<Option<String>, DeviceError> {
        Ok(ss_gpgl::parse_firmware(&self.query(TOOL_SETUP_QUERY, Duration::from_secs(1))?))
    }

    /// Waits for the reply to a registration mark search that has already been sent. The cutter
    /// answers once it has scanned the marks, which can take tens of seconds.
    pub fn wait_registration(&mut self, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Result<(), DeviceError> {
        let deadline = Instant::now() + timeout;
        let mut reply = Vec::new();
        while !reply.contains(&ETX) {
            if cancelled() {
                return Err(DeviceError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(DeviceError::Registration(format!(
                    "no reply within {} s; check that the marks are printed sharply in black and that the sheet is aligned on the mat",
                    timeout.as_secs()
                )));
            }
            reply.extend(self.transport.read(left.min(self.io_timeout))?);
        }
        ss_gpgl::parse_registration(&reply)
            .map_err(|r| DeviceError::Registration(format!("the cutter answered {r:?}; reload the sheet and try again")))
    }

    /// Polls the status until `done` accepts it. `on_status` sees every reply.
    pub fn wait_for(
        &mut self,
        done: impl Fn(&Status) -> bool,
        timeout: Duration,
        poll: Duration,
        cancelled: &dyn Fn() -> bool,
        on_status: &mut dyn FnMut(&Status),
    ) -> Result<Status, DeviceError> {
        let deadline = Instant::now() + timeout;
        loop {
            if cancelled() {
                return Err(DeviceError::Cancelled);
            }
            let s = self.status()?;
            on_status(&s);
            if done(&s) {
                return Ok(s);
            }
            if Instant::now() >= deadline {
                return Err(DeviceError::Timeout);
            }
            std::thread::sleep(poll);
        }
    }
}

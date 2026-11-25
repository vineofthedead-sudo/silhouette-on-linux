//! Writes the job to a file instead of a cutter (for inspection or sending later).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::virtual_cutter::{Query, QueryScanner};
use crate::{DeviceError, Transport};

/// Answers queries as an idle cutter would and writes only the job commands.
pub struct FileTransport {
    path: PathBuf,
    out: std::io::BufWriter<std::fs::File>,
    scanner: QueryScanner,
    replies: Vec<u8>,
}

impl FileTransport {
    pub fn create(path: &Path) -> Result<Self, DeviceError> {
        Ok(Self {
            path: path.to_path_buf(),
            out: std::io::BufWriter::new(std::fs::File::create(path)?),
            scanner: QueryScanner::default(),
            replies: Vec::new(),
        })
    }
}

impl Transport for FileTransport {
    fn write(&mut self, data: &[u8], _timeout: Duration) -> Result<usize, DeviceError> {
        let (queries, job) = self.scanner.feed(data);
        self.out.write_all(&job)?;
        for q in queries {
            match q {
                Query::Status => self.replies.extend_from_slice(b"0\x03"),
                Query::Firmware => self.replies.extend_from_slice(b" FILE\x03"),
                Query::ToolSetup => self.replies.extend_from_slice(b"none\x03"),
                Query::Initialize => {}
            }
        }
        self.out.flush()?;
        Ok(data.len())
    }

    fn read(&mut self, _timeout: Duration) -> Result<Vec<u8>, DeviceError> {
        Ok(std::mem::take(&mut self.replies))
    }

    fn description(&self) -> String {
        format!("File {}", self.path.display())
    }
}

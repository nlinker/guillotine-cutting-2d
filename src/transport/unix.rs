use std::io::Write as _;

use super::{ProgressMessage, ProgressSink};

pub struct FifoSink {
    writer: std::fs::File,
}

impl FifoSink {
    /// Create a FIFO at `path` (via `mkfifo`) and block until a reader connects.
    /// Semantics are identical to `ConnectNamedPipe` on Windows.
    pub fn new(path: &str) -> std::io::Result<Self> {
        let c_path =
            std::ffi::CString::new(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        // mkfifo returns -1 if path already exists (EEXIST) - that is fine; ignore the error.
        unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        let writer = std::fs::File::options().write(true).open(path)?;
        Ok(Self { writer })
    }
}

impl ProgressSink for FifoSink {
    fn send(&mut self, msg: ProgressMessage) -> Result<(), std::io::Error> {
        self.writer.write_all(msg.to_line().as_bytes())?;
        self.writer.flush()
    }
}

use std::io::Write as _;

use super::{ProgressMessage, ProgressSink};

pub struct StdoutSink;

impl ProgressSink for StdoutSink {
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
        print!("{}", msg.to_line());
        std::io::stdout().flush()
    }
}

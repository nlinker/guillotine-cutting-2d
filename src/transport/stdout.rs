use std::io::Write as _;

use super::{ProgressMessage, ProgressSink};

pub struct StdoutSink;

impl ProgressSink for StdoutSink {
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
        match msg {
            ProgressMessage::Progress {
                generation,
                sheets_used,
                mfg_cost,
                ..
            } => {
                eprint!("\rgen={generation:<6} sheets={sheets_used}  mfg={mfg_cost:<10}");
                std::io::stderr().flush()
            }
            _ => {
                eprintln!();
                print!("{}", msg.to_line());
                std::io::stdout().flush()
            }
        }
    }
}

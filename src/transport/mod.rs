pub mod stdout;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

use serde::Serialize;

use crate::model::{PieceSpec, SolutionSpec};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressMessage {
    Progress {
        generation: usize,
        sheets_used: usize,
        bbox_penalty: u64,
        seed: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        solution: Option<SolutionSpec>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pieces: Option<Vec<PieceSpec>>,
    },
    Done {
        seed: u64,
        sheets_used: usize,
        bbox_penalty: u64,
        solution: SolutionSpec,
        pieces: Vec<PieceSpec>,
    },
    Error {
        message: String,
    },
}

impl ProgressMessage {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("ProgressMessage serialization failed") + "\n"
    }
}

pub trait ProgressSink {
    /// Returns `Err` when the client disconnected - caller should stop the GA.
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error>;
}

pub mod stdout;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

use serde::Serialize;

use crate::model::{Piece, Solution};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressMessage {
    Progress {
        generation: usize,
        objective: i64,
        sheets_used: usize,
        seed: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        solution: Option<Solution>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pieces: Option<Vec<Piece>>,
    },
    Done {
        sheets_used: usize,
        objective: i64,
        solution: Solution,
        pieces: Vec<Piece>,
    },
    Error {
        message: String,
    },
}

impl ProgressMessage {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap() + "\n"
    }
}

pub trait ProgressSink {
    /// Returns `Err` when the client disconnected — caller should stop the GA.
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error>;
}

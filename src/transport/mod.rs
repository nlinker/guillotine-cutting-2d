pub mod stdout;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

use serde::Serialize;

use crate::model::{PieceType, SolutionSpec};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressMessage {
    Progress {
        generation: usize,
        sheets_used: usize,
        secondary_objective: u64,
        seed: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        solution: Option<SolutionSpec>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pieces: Option<Vec<PieceType>>,
    },
    /// BPC-specific progress: emitted every `progress_interval` CG iterations
    /// and immediately on each UB improvement.
    BpcProgress {
        iteration: usize,
        lb: usize,
        ub: usize,
    },
    Done {
        seed: u64,
        sheets_used: usize,
        cut_lengths: Vec<u64>,
        solution: SolutionSpec,
        pieces: Vec<PieceType>,
        #[serde(skip_serializing_if = "Option::is_none")]
        genome: Option<serde_json::Value>,
        /// BPC only: true = proven optimal, false = gap or stopped.
        #[serde(skip_serializing_if = "Option::is_none")]
        proven_optimal: Option<bool>,
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

/// Sink that discards `Progress` events and keeps only the final solution.
#[derive(Default)]
pub struct CapturingSink {
    pub solution: Option<SolutionSpec>,
}

impl ProgressSink for CapturingSink {
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
        if let ProgressMessage::Done { solution, .. } = msg {
            self.solution = Some(solution.clone());
        }
        Ok(())
    }
}

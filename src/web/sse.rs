use std::time::Instant;

use axum::response::sse::Event;
use cut::transport::{ProgressMessage, ProgressSink};
use tokio::sync::mpsc::UnboundedSender;

pub struct SseSink {
    pub tx: UnboundedSender<Event>,
    pub start: Instant,
    pub sheet_w: u32,
    pub sheet_h: u32,
}

impl ProgressSink for SseSink {
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
        let (event_name, data) = match msg {
            ProgressMessage::Progress { .. } => ("progress", msg.to_line()),
            ProgressMessage::Done { .. } => {
                let mut val = serde_json::to_value(msg).expect("ProgressMessage serialization is infallible");
                val["elapsed_secs"] = self.start.elapsed().as_secs_f64().into();
                val["sheet_w"] = self.sheet_w.into();
                val["sheet_h"] = self.sheet_h.into();
                ("done", val.to_string())
            }
            ProgressMessage::Error { .. } => ("error", msg.to_line()),
        };
        let evt = Event::default().event(event_name).data(data.trim_end());
        self.tx
            .send(evt)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "SSE client disconnected"))
    }
}

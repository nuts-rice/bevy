use crate::input::Button;
use crate::stats::StatsReport;
use serde::Serialize;
use std::collections::BTreeMap;

/// A single latency component measurement.
#[derive(Serialize, Clone)]
pub struct LatencyEntry {
    pub label: String,
    pub ms: u32,
}

#[derive(Serialize)]
pub struct Status {
    /// Currently pressed buttons (union across all viewers).
    pub buttons: Vec<Button>,
    /// Per-viewer latency breakdown: viewer_id → ordered list of components.
    pub latency: BTreeMap<String, Vec<LatencyEntry>>,
    /// Encoding and emulation performance stats.
    pub stats: StatsReport,
    /// Optional server location label (e.g. "Dallas, TX").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

pub struct StatusPublisher {
    producer: moq_json::snapshot::Producer<Status>,
}

impl StatusPublisher {
    pub fn new(broadcast: &mut moq_net::broadcast::Producer) -> anyhow::Result<Self> {
        let producer = broadcast.create_track(
            "status",
            moq_net::track::Info::default().with_timescale(moq_net::Timescale::MILLI),
        )?;

        Ok(Self {
            producer: moq_json::snapshot::Producer::new(
                producer,
                moq_json::snapshot::Config::default(),
            ),
        })
    }

    /// Publish status, a no-op if unchanged since the last call.
    pub fn publish(&mut self, status: &Status) {
        if let Err(err) = self.producer.update(status) {
            tracing::warn!(%err, "failed to publish status");
        }
    }

    pub fn finish(&mut self) {
        if let Err(e) = self.producer.finish() {
            tracing::error!(error = %e, "error when finishing status")
        }
    }
}

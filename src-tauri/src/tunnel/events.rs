//! Event contract between the tunnel core and the frontend.
//!
//! Payload shapes here are the frozen contract documented in
//! `docs/EVENTS.md`:
//!
//! | Event               | Payload                                            |
//! |---------------------|----------------------------------------------------|
//! | [`STATUS_EVENT`]    | `{ profileId, status }`                            |
//! | [`LOG_EVENT`]       | `{ profileId, index, line, level, ts }`            |
//! | [`STATS_EVENT`]     | `{ profileId, bytesUp, bytesDown, uptimeSecs }`    |
//!
//! The tunnel core stays UI-agnostic: it only knows the [`EventSink`]
//! trait. The Tauri shell provides an `AppHandle`-backed implementation;
//! tests record events instead.

use super::supervisor::TunnelStatus;
use serde::Serialize;
use uuid::Uuid;

/// Emitted on every tunnel state change, coalesced to at most one event
/// per second per tunnel.
pub const STATUS_EVENT: &str = "tunnel://status";
/// Emitted exactly once per log line.
pub const LOG_EVENT: &str = "tunnel://log";
/// Emitted once per second while a tunnel is running.
pub const STATS_EVENT: &str = "tunnel://stats";

/// Severity of a log line (serialized lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Error => "error",
        }
    }
}

/// One structured log line of a tunnel's ring buffer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Monotonic per-tunnel sequence number; resets when the tunnel is
    /// (re)started. `get_tunnel_log` resumes from the last seen index.
    pub index: u64,
    /// Unix epoch milliseconds.
    pub ts: u64,
    pub level: LogLevel,
    pub line: String,
}

/// Throughput snapshot for [`STATS_EVENT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSnapshot {
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub uptime_secs: u64,
}

impl From<&TunnelStatus> for StatsSnapshot {
    fn from(s: &TunnelStatus) -> Self {
        Self {
            bytes_up: s.bytes_up,
            bytes_down: s.bytes_down,
            uptime_secs: s.uptime_secs,
        }
    }
}

/// Consumer of tunnel events. Implemented by the Tauri shell (forwarding
/// to the webview) and by tests (recording into a vector).
pub trait EventSink: Send + Sync {
    fn emit_status(&self, profile_id: &Uuid, status: &TunnelStatus);
    fn emit_log(&self, profile_id: &Uuid, entry: &LogEntry);
    fn emit_stats(&self, profile_id: &Uuid, stats: &StatsSnapshot);
}

/// Wire payload of [`STATUS_EVENT`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload<'a> {
    pub profile_id: &'a str,
    pub status: &'a TunnelStatus,
}

/// Wire payload of [`LOG_EVENT`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPayload<'a> {
    pub profile_id: &'a str,
    pub index: u64,
    pub line: &'a str,
    pub level: LogLevel,
    pub ts: u64,
}

/// Wire payload of [`STATS_EVENT`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsPayload<'a> {
    pub profile_id: &'a str,
    #[serde(flatten)]
    pub stats: &'a StatsSnapshot,
}

impl<'a> LogPayload<'a> {
    pub fn new(profile_id: &'a str, entry: &'a LogEntry) -> Self {
        Self {
            profile_id,
            index: entry.index,
            line: &entry.line,
            level: entry.level,
            ts: entry.ts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_status() -> TunnelStatus {
        TunnelStatus {
            state: "connected".to_string(),
            server_kind: Some("Spore".to_string()),
            local_address: "127.0.0.1:25565".to_string(),
            remote_address: Some("bore.example.com:10000".to_string()),
            assigned_remote_port: Some(10000),
            uptime_secs: 12,
            bytes_up: 100,
            bytes_down: 200,
            reconnects: 1,
            last_error: None,
            logs: vec!["hello".to_string()],
        }
    }

    #[test]
    fn log_entry_serializes_in_contract_shape() {
        let entry = LogEntry {
            index: 7,
            ts: 1_700_000_000_123u64,
            level: LogLevel::Error,
            line: "tunnel down".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({
                "index": 7,
                "ts": 1_700_000_000_123u64,
                "level": "error",
                "line": "tunnel down"
            })
        );
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }

    #[test]
    fn status_payload_is_camelcase_with_nested_status() {
        let id = "6f2a2c3e-1111-4000-8000-000000000000";
        let status = sample_status();
        let payload = StatusPayload {
            profile_id: id,
            status: &status,
        };
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["profileId"], json!(id));
        assert_eq!(v["status"]["state"], json!("connected"));
        assert_eq!(v["status"]["serverKind"], json!("Spore"));
        assert_eq!(v["status"]["remoteAddress"], json!("bore.example.com:10000"));
        assert_eq!(v["status"]["assignedRemotePort"], json!(10000));
        assert_eq!(v["status"]["uptimeSecs"], json!(12));
        assert_eq!(v["status"]["bytesUp"], json!(100));
        assert_eq!(v["status"]["bytesDown"], json!(200));
        assert_eq!(v["status"]["reconnects"], json!(1));
        assert_eq!(v["status"]["lastError"], json!(null));
        assert_eq!(v["status"]["logs"], json!(["hello"]));
    }

    #[test]
    fn log_payload_is_camelcase_with_line_fields() {
        let id = "6f2a2c3e-2222-4000-8000-000000000000";
        let entry = LogEntry {
            index: 3,
            ts: 42_000,
            level: LogLevel::Info,
            line: "connecting".to_string(),
        };
        let payload = LogPayload::new(id, &entry);
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            json!({
                "profileId": id,
                "index": 3,
                "line": "connecting",
                "level": "info",
                "ts": 42_000,
            })
        );
    }

    #[test]
    fn stats_payload_flattens_snapshot_fields() {
        let id = "6f2a2c3e-3333-4000-8000-000000000000";
        let status = sample_status();
        let payload = StatsPayload {
            profile_id: id,
            stats: &StatsSnapshot::from(&status),
        };
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            json!({
                "profileId": id,
                "bytesUp": 100,
                "bytesDown": 200,
                "uptimeSecs": 12,
            })
        );
    }
}

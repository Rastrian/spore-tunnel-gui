//! Tauri-backed [`EventSink`]: forwards tunnel events to the webview.
//!
//! `Emitter::emit` requires `Clone` payloads, but the frozen event payload
//! types borrow from the caller; each emitter therefore serializes once
//! and uses `emit_str`, which puts the identical JSON on the wire.

use spore_tunnel_gui::tunnel::events::{
    EventSink, LogEntry, LogPayload, StatsPayload, StatsSnapshot, StatusPayload, LOG_EVENT,
    STATS_EVENT, STATUS_EVENT,
};
use spore_tunnel_gui::tunnel::supervisor::TunnelStatus;
use tauri::Emitter;
use uuid::Uuid;

pub struct AppEventSink {
    app: tauri::AppHandle,
}

impl AppEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }

    fn emit_json<T: serde::Serialize>(&self, event: &str, payload: &T) {
        if let Ok(json) = serde_json::to_string(payload) {
            let _ = self.app.emit_str(event, json);
        }
    }
}

impl EventSink for AppEventSink {
    fn emit_status(&self, profile_id: &Uuid, status: &TunnelStatus) {
        let id = profile_id.to_string();
        self.emit_json(
            STATUS_EVENT,
            &StatusPayload {
                profile_id: &id,
                status,
            },
        );
    }

    fn emit_log(&self, profile_id: &Uuid, entry: &LogEntry) {
        let id = profile_id.to_string();
        self.emit_json(LOG_EVENT, &LogPayload::new(&id, entry));
    }

    fn emit_stats(&self, profile_id: &Uuid, stats: &StatsSnapshot) {
        let id = profile_id.to_string();
        self.emit_json(
            STATS_EVENT,
            &StatsPayload {
                profile_id: &id,
                stats,
            },
        );
    }
}

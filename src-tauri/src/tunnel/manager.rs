//! Multi-tunnel manager: one supervised tunnel per profile, plus the
//! per-tunnel event pump that forwards supervisor changes to an
//! [`EventSink`] under the frozen event contract.
//!
//! Event pump guarantees (per tunnel):
//! * the CURRENT status is emitted on entry, so the initial `starting`
//!   event is never lost — the status watch is subscribed BEFORE the
//!   supervisor is started;
//! * log entries are flushed immediately on every status change
//!   (`since == None` means "everything currently in the ring");
//! * status events are coalesced to at most one per second per tunnel
//!   (trailing edge: the latest status is kept and flushed exactly one
//!   second after the previous emit);
//! * stats are emitted on a 1 s ticker while the tunnel is running;
//! * a `stopped` status is emitted immediately and ends the pump.
//!
//! Stopped tunnels KEEP their manager entry (supervisor + logs) until the
//! next start replaces it, so status and logs stay queryable.

use super::client::TunnelConfig;
use super::events::{EventSink, LogEntry, StatsSnapshot};
use super::supervisor::{SupervisorConfig, TunnelStatus, TunnelSupervisor};
use crate::config::Profile;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Minimum spacing between two `tunnel://status` events per tunnel.
const STATUS_COALESCE_WINDOW: Duration = Duration::from_secs(1);
/// Cadence of `tunnel://stats` while a tunnel is running.
const STATS_EVERY: Duration = Duration::from_secs(1);

/// Owns every running (and recently stopped) tunnel.
pub struct TunnelManager {
    tunnels: Mutex<HashMap<Uuid, RunningTunnel>>,
    sink: Arc<dyn EventSink>,
}

struct RunningTunnel {
    profile: Profile,
    supervisor: Arc<TunnelSupervisor>,
    /// Event pump task; taken (aborted) on stop. The entry itself is kept
    /// until the next start so status/logs remain queryable.
    pump: Option<JoinHandle<()>>,
    /// Set by [`TunnelManager::stop`] in the same critical section that
    /// takes the pump; cleared on (re)start. While `false` the tunnel is
    /// considered live — `supervisor.start()` returns before the spawned
    /// task reports `starting`, so `is_running()` alone would race.
    stopped: bool,
}

impl RunningTunnel {
    /// A restart (start on an existing entry) is allowed once the tunnel
    /// is dead: stopped by the user, or terminally failed.
    fn accepts_restart(&self) -> bool {
        self.stopped || matches!(self.supervisor.status().state.as_str(), "failed" | "stopped")
    }
}

impl TunnelManager {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            tunnels: Mutex::new(HashMap::new()),
            sink,
        }
    }

    /// Start (or restart) the tunnel for `profile`.
    ///
    /// Errors when this profile already runs a tunnel. A restart replaces
    /// the previous entry with a fresh supervisor — logs restart from
    /// index 0 (documented behavior).
    pub async fn start(&self, profile: Profile, secret: String) -> Result<(), String> {
        let mut tunnels = self.tunnels.lock().await;
        if tunnels.get(&profile.id).is_some_and(|t| !t.accepts_restart()) {
            return Err("Tunnel for this profile is already running.".to_string());
        }

        let mut cfg = SupervisorConfig::new(
            TunnelConfig {
                server: profile.server_host.clone(),
                control_port: profile.server_port,
                remote_port: profile.remote_port,
            },
            secret,
            profile.local_host.clone(),
            profile.local_port,
        );
        cfg.auto_reconnect = profile.auto_reconnect;

        // CRITICAL ordering: subscribe the pump to the status watch BEFORE
        // starting the supervisor, so no change can slip by; the pump also
        // emits the current status on entry, so `starting` is never lost.
        let supervisor = Arc::new(TunnelSupervisor::new());
        let status_rx = supervisor.subscribe();
        let pump = tokio::spawn(pump_task(
            profile.id,
            supervisor.clone(),
            self.sink.clone(),
            status_rx,
        ));
        if let Err(e) = supervisor.start(cfg).await {
            pump.abort();
            return Err(e);
        }

        let id = profile.id;
        let entry = RunningTunnel {
            profile,
            supervisor,
            pump: Some(pump),
            stopped: false,
        };
        if let Some(old) = tunnels.insert(id, entry) {
            if let Some(old_pump) = old.pump {
                old_pump.abort();
            }
        }
        Ok(())
    }

    /// Stop the tunnel for `id`. Returns `false` when the id is unknown.
    ///
    /// The final `stopped` status is emitted through the sink directly —
    /// the last event must not depend on the pump. The entry is kept so
    /// status/logs stay queryable until the next start.
    pub async fn stop(&self, id: &Uuid) -> bool {
        // Mark the entry stopped AND take the pump in one critical
        // section, so a concurrent start() that replaces the entry can
        // never abort the fresh tunnel's pump.
        let (supervisor, pump) = match self.tunnels.lock().await.get_mut(id) {
            Some(t) => {
                t.stopped = true;
                (t.supervisor.clone(), t.pump.take())
            }
            None => return false,
        };
        supervisor.stop().await;
        if let Some(pump) = pump {
            pump.abort();
        }
        self.sink.emit_status(id, &supervisor.status());
        true
    }

    /// Stop every tunnel managed here.
    pub async fn stop_all(&self) {
        let ids: Vec<Uuid> = self.tunnels.lock().await.keys().copied().collect();
        for id in ids {
            self.stop(&id).await;
        }
    }

    /// The profile backing a tunnel entry (stopped entries included).
    pub async fn profile_of(&self, id: &Uuid) -> Option<Profile> {
        self.tunnels.lock().await.get(id).map(|t| t.profile.clone())
    }

    pub async fn status_of(&self, id: &Uuid) -> Option<TunnelStatus> {
        self.tunnels
            .lock()
            .await
            .get(id)
            .map(|t| t.supervisor.status())
    }

    pub async fn all_statuses(&self) -> Vec<(Uuid, TunnelStatus)> {
        self.tunnels
            .lock()
            .await
            .iter()
            .map(|(id, t)| (*id, t.supervisor.status()))
            .collect()
    }

    /// Whether the tunnel for `id` is starting or connected. Unknown ids
    /// are simply not running.
    pub async fn is_running(&self, id: &Uuid) -> bool {
        self.tunnels
            .lock()
            .await
            .get(id)
            .is_some_and(|t| t.supervisor.is_running())
    }

    /// Log entries for `id`. `since == None` returns the whole ring
    /// (backfill); `Some(n)` returns only entries with index > n.
    pub async fn logs_of(&self, id: &Uuid, since: Option<u64>) -> Option<Vec<LogEntry>> {
        self.tunnels.lock().await.get(id).map(|t| match since {
            None => t.supervisor.log_snapshot(),
            Some(n) => t.supervisor.log_entries_since(n),
        })
    }

    /// Wait (up to `within`) until the tunnel for `id` is in one of
    /// `states`, then return the latest status. `None` when the id is
    /// unknown to this manager.
    pub async fn wait_for(
        &self,
        id: &Uuid,
        states: &[&str],
        within: Duration,
    ) -> Option<TunnelStatus> {
        let supervisor = self.tunnels.lock().await.get(id).map(|t| t.supervisor.clone())?;
        Some(supervisor.wait_for(states, within).await)
    }
}

/// Per-tunnel event pump — see the module docs for the guarantees.
async fn pump_task(
    profile_id: Uuid,
    supervisor: Arc<TunnelSupervisor>,
    sink: Arc<dyn EventSink>,
    mut status_rx: watch::Receiver<TunnelStatus>,
) {
    let mut stats = tokio::time::interval(STATS_EVERY);
    stats.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    stats.tick().await; // consume the immediate first tick: cadence is 1/s

    let mut last_log: Option<u64> = None; // None = flush the whole ring
    let mut pending_flush: Option<tokio::time::Instant> = None;

    // Emit the current status on entry so the initial `starting` event is
    // never lost; this also starts the coalescing window.
    sink.emit_status(&profile_id, &supervisor.status());
    let mut last_status_emit = tokio::time::Instant::now();

    loop {
        tokio::select! {
            changed = status_rx.changed() => {
                if changed.is_err() {
                    break; // supervisor dropped — nothing left to pump
                }
                let status = status_rx.borrow_and_update().clone();

                // 1) New log lines go out immediately, never coalesced.
                let entries = match last_log {
                    Some(since) => supervisor.log_entries_since(since),
                    None => supervisor.log_snapshot(),
                };
                for entry in &entries {
                    sink.emit_log(&profile_id, entry);
                }
                last_log = entries.last().map(|e| e.index);

                // 2) Status. `stopped` is final: emit immediately, end pump.
                if status.state == "stopped" {
                    sink.emit_status(&profile_id, &status);
                    break;
                }
                // Otherwise coalesce to at most one event per second:
                // emit now when the previous emit is old enough, else keep
                // the latest pending and flush it exactly one second after
                // the previous emit.
                if last_status_emit.elapsed() >= STATUS_COALESCE_WINDOW {
                    sink.emit_status(&profile_id, &supervisor.status());
                    last_status_emit = tokio::time::Instant::now();
                    pending_flush = None;
                } else {
                    pending_flush = Some(last_status_emit + STATUS_COALESCE_WINDOW);
                }
            }

            _ = stats.tick() => {
                if supervisor.is_running() {
                    let status = supervisor.status();
                    sink.emit_stats(&profile_id, &StatsSnapshot::from(&status));
                }
            }

            // Armed only while a status is pending: an always-ready timer
            // arm would busy-loop the select.
            _ = tokio::time::sleep_until(pending_flush.unwrap_or_else(
                || tokio::time::Instant::now() + STATUS_COALESCE_WINDOW,
            )), if pending_flush.is_some() => {
                sink.emit_status(&profile_id, &supervisor.status());
                last_status_emit = tokio::time::Instant::now();
                pending_flush = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::events::LogLevel;
    use crate::tunnel::mock_server::{mock, spawn_reply_once_service};
    use std::sync::Mutex as StdMutex;
    use std::time::Instant;

    const WAIT: Duration = Duration::from_secs(3);

    type SharedEvents = Arc<StdMutex<Vec<Recorded>>>;

    #[derive(Debug, Clone)]
    enum RecordedEvent {
        Status(TunnelStatus),
        Log(LogEntry),
        Stats(StatsSnapshot),
    }

    #[derive(Debug, Clone)]
    struct Recorded {
        profile_id: Uuid,
        event: RecordedEvent,
    }

    struct RecordingSink(SharedEvents);

    impl EventSink for RecordingSink {
        fn emit_status(&self, profile_id: &Uuid, status: &TunnelStatus) {
            self.0.lock().unwrap().push(Recorded {
                profile_id: *profile_id,
                event: RecordedEvent::Status(status.clone()),
            });
        }

        fn emit_log(&self, profile_id: &Uuid, entry: &LogEntry) {
            self.0.lock().unwrap().push(Recorded {
                profile_id: *profile_id,
                event: RecordedEvent::Log(entry.clone()),
            });
        }

        fn emit_stats(&self, profile_id: &Uuid, stats: &StatsSnapshot) {
            self.0.lock().unwrap().push(Recorded {
                profile_id: *profile_id,
                event: RecordedEvent::Stats(*stats),
            });
        }
    }

    fn recording_sink() -> (Arc<RecordingSink>, SharedEvents) {
        let events: SharedEvents = Arc::new(StdMutex::new(Vec::new()));
        (Arc::new(RecordingSink(events.clone())), events)
    }

    fn profile_for(control_port: u16, local_port: u16) -> Profile {
        Profile {
            id: Uuid::new_v4(),
            name: format!("profile-{control_port}"),
            server_host: "127.0.0.1".to_string(),
            server_port: control_port,
            local_host: "127.0.0.1".to_string(),
            local_port,
            remote_port: 0,
            autostart: false,
            auto_reconnect: true,
        }
    }

    /// Poll a sync predicate until it yields a value or the deadline hits.
    async fn eventually<T>(within: Duration, mut pred: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + within;
        loop {
            if let Some(v) = pred() {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "condition not met within {within:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn statuses_of(events: &SharedEvents, id: &Uuid) -> Vec<TunnelStatus> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.profile_id == *id)
            .filter_map(|r| match &r.event {
                RecordedEvent::Status(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn logs_of_events(events: &SharedEvents, id: &Uuid) -> Vec<LogEntry> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.profile_id == *id)
            .filter_map(|r| match &r.event {
                RecordedEvent::Log(e) => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    async fn connected(manager: &TunnelManager, id: &Uuid) -> TunnelStatus {
        manager
            .wait_for(id, &["connected"], WAIT)
            .await
            .unwrap_or_else(|| panic!("no tunnel entry for {id}"))
    }

    #[tokio::test]
    async fn two_tunnels_run_independently() {
        let mock_a = mock().start().await.unwrap();
        let mock_b = mock().start().await.unwrap();
        let (local_a, _ha) = spawn_reply_once_service().await.unwrap();
        let (local_b, _hb) = spawn_reply_once_service().await.unwrap();
        let (sink, _events) = recording_sink();
        let manager = TunnelManager::new(sink);

        let profile_a = profile_for(mock_a.control_addr().port(), local_a.port());
        let profile_b = profile_for(mock_b.control_addr().port(), local_b.port());
        manager.start(profile_a.clone(), String::new()).await.unwrap();
        manager.start(profile_b.clone(), String::new()).await.unwrap();

        let status_a = connected(&manager, &profile_a.id).await;
        let status_b = connected(&manager, &profile_b.id).await;
        // Entries carry their own profile.
        assert_eq!(manager.profile_of(&profile_a.id).await, Some(profile_a.clone()));
        assert_eq!(manager.profile_of(&profile_b.id).await, Some(profile_b.clone()));
        // Each tunnel got its own server and its own assigned port.
        assert_ne!(mock_a.assigned_port(), mock_b.assigned_port());
        assert_eq!(status_a.assigned_remote_port, Some(mock_a.assigned_port()));
        assert_eq!(status_b.assigned_remote_port, Some(mock_b.assigned_port()));
        assert_eq!(
            status_a.remote_address.as_deref(),
            Some(format!("127.0.0.1:{}", mock_a.assigned_port()).as_str())
        );

        // Stopping A leaves B connected.
        assert!(manager.stop(&profile_a.id).await);
        let stopped = manager
            .wait_for(&profile_a.id, &["stopped"], WAIT)
            .await
            .unwrap();
        assert_eq!(stopped.state, "stopped");
        assert_eq!(manager.status_of(&profile_b.id).await.unwrap().state, "connected");

        // stop_all drains the running set entirely.
        manager.stop_all().await;
        for id in [profile_a.id, profile_b.id] {
            assert!(!manager.is_running(&id).await);
            assert_eq!(manager.status_of(&id).await.unwrap().state, "stopped");
        }
        mock_a.stop().await;
        mock_b.stop().await;
    }

    #[tokio::test]
    async fn event_stream_records_lifecycle() {
        let mock = mock().start().await.unwrap();
        let (local, _h) = spawn_reply_once_service().await.unwrap();
        let (sink, events) = recording_sink();
        let manager = TunnelManager::new(sink);
        let profile = profile_for(mock.control_addr().port(), local.port());
        manager.start(profile.clone(), String::new()).await.unwrap();
        connected(&manager, &profile.id).await;

        // Status stream: the pump's entry emit fires first. What it shows
        // depends on how far the localhost handshake got before the pump's
        // first poll: `idle` (supervisor.start ran, task not connected
        // yet — a fast connect goes straight to `connected`, never through
        // `starting`), `starting` (a connect attempt failed and is
        // retrying), or already `connected`. No change is ever missed: the
        // watch was subscribed before the supervisor started. A later
        // `connected` may also arrive up to 1 s late (coalescing).
        let statuses = eventually(WAIT + Duration::from_secs(1), || {
            let s = statuses_of(&events, &profile.id);
            s.iter().any(|x| x.state == "connected").then_some(s)
        })
        .await;
        assert!(
            matches!(
                statuses.first().expect("at least one status").state.as_str(),
                "idle" | "starting" | "connected"
            ),
            "statuses: {statuses:?}"
        );
        let connected_status = statuses
            .iter()
            .find(|s| s.state == "connected")
            .expect("connected status");
        assert_eq!(connected_status.assigned_remote_port, Some(mock.assigned_port()));
        assert_eq!(connected_status.server_kind.as_deref(), Some("Bore"));

        // Log events: strictly increasing indexes, real timestamps.
        let logs = eventually(WAIT, || {
            let l = logs_of_events(&events, &profile.id);
            (!l.is_empty()).then_some(l)
        })
        .await;
        assert!(
            logs.windows(2).all(|w| w[0].index < w[1].index),
            "log indexes must increase: {logs:?}"
        );
        assert!(logs.iter().all(|e| e.ts > 0));
        assert!(logs
            .iter()
            .any(|e| e.level == LogLevel::Info && e.line.contains("connecting to")));

        // Stats tick at ~1 Hz while running, carrying live counters.
        let stats = eventually(WAIT, || {
            let snapshots: Vec<StatsSnapshot> = events
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.profile_id == profile.id)
                .filter_map(|r| match &r.event {
                    RecordedEvent::Stats(s) => Some(*s),
                    _ => None,
                })
                .collect();
            (!snapshots.is_empty()).then_some(snapshots)
        })
        .await;
        assert!(
            stats.iter().all(|s| s.uptime_secs <= WAIT.as_secs() + 1),
            "stats: {stats:?}"
        );

        manager.stop_all().await;
        mock.stop().await;
    }

    #[tokio::test]
    async fn stop_emits_final_stopped_and_keeps_the_entry_queryable() {
        let mock = mock().start().await.unwrap();
        let (local, _h) = spawn_reply_once_service().await.unwrap();
        let (sink, events) = recording_sink();
        let manager = TunnelManager::new(sink);
        let profile = profile_for(mock.control_addr().port(), local.port());
        manager.start(profile.clone(), String::new()).await.unwrap();
        connected(&manager, &profile.id).await;

        assert!(manager.stop(&profile.id).await);
        assert!(!manager.is_running(&profile.id).await);
        // Entry (and its logs) stay queryable until the next start.
        assert_eq!(
            manager.status_of(&profile.id).await.unwrap().state,
            "stopped"
        );
        let logs = manager.logs_of(&profile.id, None).await.unwrap();
        assert!(logs.iter().any(|e| e.line == "stopped by user"));
        // Incremental read: nothing newer than the newest index.
        let last = logs.last().unwrap().index;
        assert!(manager
            .logs_of(&profile.id, Some(last))
            .await
            .unwrap()
            .is_empty());

        // The last status event for this tunnel is `stopped` (emitted by
        // stop() directly, not by the aborted pump).
        let statuses = eventually(WAIT, || {
            let s = statuses_of(&events, &profile.id);
            let stopped_last =
                matches!(s.last(), Some(last) if last.state == "stopped");
            stopped_last.then_some(s)
        })
        .await;
        assert_eq!(statuses.last().unwrap().state, "stopped");

        // Restart replaces the entry: fresh supervisor, logs from index 0.
        manager.start(profile.clone(), String::new()).await.unwrap();
        connected(&manager, &profile.id).await;
        let fresh = manager.logs_of(&profile.id, None).await.unwrap();
        assert_eq!(fresh.first().map(|e| e.index), Some(0));
        assert!(manager.stop(&profile.id).await);
        mock.stop().await;
    }

    #[tokio::test]
    async fn starting_twice_for_the_same_profile_is_rejected() {
        let mock = mock().start().await.unwrap();
        let (local, _h) = spawn_reply_once_service().await.unwrap();
        let (sink, _events) = recording_sink();
        let manager = TunnelManager::new(sink);
        let profile = profile_for(mock.control_addr().port(), local.port());

        manager.start(profile.clone(), String::new()).await.unwrap();
        let err = manager.start(profile.clone(), String::new()).await.unwrap_err();
        assert_eq!(err, "Tunnel for this profile is already running.");

        manager.stop_all().await;
        mock.stop().await;
    }

    #[tokio::test]
    async fn unknown_ids_are_not_running_and_not_stoppable() {
        let (sink, _events) = recording_sink();
        let manager = TunnelManager::new(sink);
        let id = Uuid::new_v4();

        assert!(!manager.is_running(&id).await);
        assert!(!manager.stop(&id).await);
        assert!(manager.status_of(&id).await.is_none());
        assert!(manager.logs_of(&id, None).await.is_none());
        assert!(manager.wait_for(&id, &["connected"], WAIT).await.is_none());
        assert!(manager.all_statuses().await.is_empty());
    }
}

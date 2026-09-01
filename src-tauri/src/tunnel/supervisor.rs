//! Tunnel supervisor: owns the control loop, forwarder tasks, reconnects,
//! status and logs for ONE tunnel.
//!
//! Liveness policy (both dialects): real bore and Spore servers push a
//! keepalive frame on the control connection roughly every 500 ms — bore
//! sends the bare string `"Heartbeat"`, Spore sends `"Heartbeat"` too (our
//! mock models Spore's keepalive as `"Ack"`, which is consumed equally).
//! Silence for longer than `ack_window` (default 10 s, ~20 missed beats)
//! declares the tunnel dead, as do EOF and IO errors (TCP health). The
//! client never originates keepalive frames: real bore servers decode
//! `ClientMessage` as a closed enum and drop unknown variants, and Spore
//! expects client silence on the control plane.
//!
//! On death the session is fully torn down — control connection dropped and
//! every forwarder task aborted — before `auto_reconnect` dials again with
//! exponential backoff (5 s → 60 s cap, ±20% jitter).

use super::client::{self, ServerKind, TunnelClient, TunnelConfig};
use super::events::{LogEntry, LogLevel};
use super::forward::{self, ByteCounters};
use super::protocol::{FrameReader, ServerMessage};
use super::TunnelError;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::timeout;

/// Cap of the in-memory log ring surfaced through [`TunnelStatus`].
pub const MAX_LOG_LINES: usize = 1024;
/// Grace period for the supervisor task to exit on `stop()` before abort.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// Tunables and endpoints for the supervised tunnel.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub tunnel: TunnelConfig,
    pub secret: String,
    pub local_host: String,
    pub local_port: u16,
    pub auto_reconnect: bool,
    /// Keepalive window (default 10 s): no server Heartbeat/Ack within it
    /// declares the tunnel dead. Real servers beat every ~500 ms.
    pub ack_window: Duration,
    /// First reconnect delay (default 5 s), doubling per death.
    pub backoff_base: Duration,
    /// Reconnect delay ceiling (default 60 s).
    pub backoff_max: Duration,
}

impl SupervisorConfig {
    pub fn new(
        tunnel: TunnelConfig,
        secret: impl Into<String>,
        local_host: impl Into<String>,
        local_port: u16,
    ) -> Self {
        Self {
            tunnel,
            secret: secret.into(),
            local_host: local_host.into(),
            local_port,
            auto_reconnect: true,
            ack_window: Duration::from_secs(10),
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(60),
        }
    }

    fn local_address(&self) -> String {
        format!("{}:{}", self.local_host, self.local_port)
    }
}

/// Snapshot of everything the UI shows about the tunnel.
///
/// Serialized camelCase — this shape is part of the `tunnel://status`
/// event contract (`docs/EVENTS.md`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    /// `idle` | `starting` | `connected` | `failed` | `stopped`
    /// (a reconnecting tunnel reports `starting`).
    pub state: String,
    /// `Some("Bore" | "Spore")` once the handshake identified the server.
    pub server_kind: Option<String>,
    pub local_address: String,
    pub remote_address: Option<String>,
    pub assigned_remote_port: Option<u16>,
    pub uptime_secs: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub reconnects: u32,
    pub last_error: Option<String>,
    pub logs: Vec<String>,
}

impl Default for TunnelStatus {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            server_kind: None,
            local_address: "127.0.0.1:25565".to_string(),
            remote_address: None,
            assigned_remote_port: None,
            uptime_secs: 0,
            bytes_up: 0,
            bytes_down: 0,
            reconnects: 0,
            last_error: None,
            logs: Vec::new(),
        }
    }
}

/// Internal state machine; `Connecting` covers both the initial connect and
/// reconnects so the frontend keeps its five known states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Connecting,
    Connected,
    Failed,
    Stopped,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "starting",
            Self::Connected => "connected",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone)]
struct InnerState {
    state: State,
    kind: Option<ServerKind>,
    local_addr: String,
    remote_addr: Option<String>,
    assigned_port: Option<u16>,
    connected_at: Option<Instant>,
    reconnects: u32,
    last_error: Option<String>,
    /// Ring of the last [`MAX_LOG_LINES`] structured entries.
    log_ring: VecDeque<LogEntry>,
    /// Index of the NEXT log entry; resets when the tunnel restarts.
    next_log_index: u64,
}

impl Default for InnerState {
    fn default() -> Self {
        Self {
            state: State::Idle,
            kind: None,
            local_addr: "127.0.0.1:25565".to_string(),
            remote_addr: None,
            assigned_port: None,
            connected_at: None,
            reconnects: 0,
            last_error: None,
            log_ring: VecDeque::new(),
            next_log_index: 0,
        }
    }
}

impl InnerState {
    fn push_log(&mut self, level: LogLevel, line: String) {
        if self.log_ring.len() >= MAX_LOG_LINES {
            self.log_ring.pop_front();
        }
        let index = self.next_log_index;
        self.next_log_index = index.saturating_add(1);
        self.log_ring.push_back(LogEntry {
            index,
            ts: now_ms(),
            level,
            line,
        });
    }
}

/// Unix epoch milliseconds (0 if the clock is before the epoch).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct Shared {
    inner: Mutex<InnerState>,
    status_tx: watch::Sender<TunnelStatus>,
    counters: Arc<ByteCounters>,
    active_conns: Arc<AtomicUsize>,
    stop_tx: watch::Sender<bool>,
}

impl Shared {
    fn mutate(&self, f: impl FnOnce(&mut InnerState)) {
        let status = {
            let mut s = self.inner.lock().unwrap();
            f(&mut s);
            build_status(&s, &self.counters)
        };
        self.status_tx.send_replace(status);
    }

    fn log(&self, level: LogLevel, line: String) {
        self.mutate(|s| s.push_log(level, line));
    }

    fn stopping(&self) -> bool {
        *self.stop_tx.borrow()
    }
}

fn build_status(s: &InnerState, counters: &ByteCounters) -> TunnelStatus {
    let (bytes_up, bytes_down) = counters.snapshot();
    TunnelStatus {
        state: s.state.as_str().to_string(),
        server_kind: s.kind.map(|k| k.as_str().to_string()),
        local_address: s.local_addr.clone(),
        remote_address: s.remote_addr.clone(),
        assigned_remote_port: s.assigned_port,
        uptime_secs: s.connected_at.map(|t| t.elapsed().as_secs()).unwrap_or(0),
        bytes_up,
        bytes_down,
        reconnects: s.reconnects,
        last_error: s.last_error.clone(),
        logs: s.log_ring.iter().map(|e| e.line.clone()).collect(),
    }
}

/// Owns the lifecycle of one tunnel.
pub struct TunnelSupervisor {
    shared: Arc<Shared>,
    status_rx: watch::Receiver<TunnelStatus>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Default for TunnelSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelSupervisor {
    pub fn new() -> Self {
        let (status_tx, status_rx) = watch::channel(TunnelStatus::default());
        let (stop_tx, _) = watch::channel(false);
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(InnerState::default()),
                status_tx,
                counters: Arc::new(ByteCounters::new()),
                active_conns: Arc::new(AtomicUsize::new(0)),
                stop_tx,
            }),
            status_rx,
            task: tokio::sync::Mutex::new(None),
        }
    }

    /// Fresh snapshot (uptime and byte counters are read live).
    pub fn status(&self) -> TunnelStatus {
        let s = self.shared.inner.lock().unwrap();
        build_status(&s, &self.shared.counters)
    }

    /// Live status stream (e.g. for event-driven UIs).
    pub fn subscribe(&self) -> watch::Receiver<TunnelStatus> {
        self.status_rx.clone()
    }

    /// Ring entries with `index` strictly greater than `since`, oldest
    /// first — the incremental stream behind `tunnel://log` and
    /// `get_tunnel_log(profileId, sinceIndex: Some(n))`. Entries evicted
    /// from the ring are skipped silently.
    pub fn log_entries_since(&self, since: u64) -> Vec<LogEntry> {
        let s = self.shared.inner.lock().unwrap();
        s.log_ring
            .iter()
            .filter(|e| e.index > since)
            .cloned()
            .collect()
    }

    /// Every entry currently in the ring, oldest first (the
    /// `sinceIndex: null` backfill of `get_tunnel_log`).
    pub fn log_snapshot(&self) -> Vec<LogEntry> {
        self.shared.inner.lock().unwrap().log_ring.iter().cloned().collect()
    }

    /// Index of the newest ring entry (0 when nothing is logged yet).
    pub fn last_log_index(&self) -> u64 {
        self.shared
            .inner
            .lock()
            .unwrap()
            .log_ring
            .back()
            .map(|e| e.index)
            .unwrap_or(0)
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.shared.inner.lock().unwrap().state,
            State::Connecting | State::Connected
        )
    }

    /// Forwarder tasks currently alive (also used by tests to prove that
    /// teardown leaves nothing behind).
    pub fn active_connections(&self) -> usize {
        self.shared.active_conns.load(Ordering::Relaxed)
    }

    /// Start (or restart) the supervised tunnel.
    pub async fn start(&self, cfg: SupervisorConfig) -> Result<(), String> {
        if self.is_running() {
            return Err("Tunnel is already running.".to_string());
        }
        let _ = self.shared.stop_tx.send(false);
        self.shared.counters.up.store(0, Ordering::Relaxed);
        self.shared.counters.down.store(0, Ordering::Relaxed);
        self.shared.mutate(|s| {
            *s = InnerState {
                local_addr: cfg.local_address(),
                ..InnerState::default()
            };
        });
        self.shared.log(
            LogLevel::Info,
            format!("connecting to {}", cfg.tunnel.control_addr()),
        );

        let handle = tokio::spawn(supervisor_task(cfg, self.shared.clone()));
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    /// Stop the tunnel and tear everything down.
    pub async fn stop(&self) {
        let _ = self.shared.stop_tx.send(true);
        let handle = self.task.lock().await.take();
        if let Some(mut handle) = handle {
            if timeout(STOP_GRACE, &mut handle).await.is_err() {
                handle.abort();
            }
        }
        self.shared.mutate(|s| {
            s.state = State::Stopped;
            s.connected_at = None;
        });
        self.shared.log(LogLevel::Info, "stopped by user".to_string());
    }

    /// Wait (up to `within`) until `state` is one of `states`, then return
    /// the latest status either way.
    pub async fn wait_for(&self, states: &[&str], within: Duration) -> TunnelStatus {
        let mut rx = self.status_rx.clone();
        let deadline = Instant::now() + within;
        loop {
            if states.contains(&self.status().state.as_str()) {
                return self.status();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self.status();
            }
            let _ = timeout(remaining, rx.changed()).await;
        }
    }
}

async fn supervisor_task(cfg: SupervisorConfig, shared: Arc<Shared>) {
    let mut deaths: u32 = 0;
    let mut rng = Jitter::new();

    loop {
        match TunnelClient::connect(&cfg.tunnel, &cfg.secret).await {
            Ok(conn) => {
                deaths = 0;
                let (port, info, reader, writer) = conn.into_parts();
                let remote = format!("{}:{}", cfg.tunnel.server, port);
                shared.mutate(|s| {
                    s.state = State::Connected;
                    s.kind = Some(info.kind);
                    s.remote_addr = Some(remote.clone());
                    s.assigned_port = Some(port);
                    s.connected_at = Some(Instant::now());
                    s.last_error = None;
                });
                shared.log(
                    LogLevel::Info,
                    format!(
                        "connected to {} ({}) — listening at {remote}",
                        cfg.tunnel.control_addr(),
                        info.kind.as_str()
                    ),
                );

                let outcome = run_session(&cfg, reader, writer, &shared).await;
                if shared.stopping() {
                    break;
                }
                let err = match outcome {
                    Ok(()) => break, // stopped without error
                    Err(e) => e,
                };
                shared.mutate(|s| {
                    s.reconnects += 1;
                    s.last_error = Some(err.to_string());
                    s.state = State::Connecting;
                    s.remote_addr = None;
                    s.assigned_port = None;
                    s.connected_at = None;
                });
                shared.log(LogLevel::Error, format!("tunnel down: {err}"));
                if !cfg.auto_reconnect {
                    shared.mutate(|s| s.state = State::Failed);
                    break;
                }
            }
            Err(err) => {
                shared.mutate(|s| {
                    s.last_error = Some(err.to_string());
                    s.state = if cfg.auto_reconnect {
                        State::Connecting
                    } else {
                        State::Failed
                    };
                });
                shared.log(LogLevel::Error, format!("connect failed: {err}"));
                if !cfg.auto_reconnect || shared.stopping() {
                    break;
                }
            }
        }

        let delay = backoff_delay(deaths, cfg.backoff_base, cfg.backoff_max, &mut rng);
        deaths = deaths.saturating_add(1);
        shared.log(
            LogLevel::Info,
            format!("reconnecting in {} ms", delay.as_millis()),
        );
        let mut stop_rx = shared.stop_tx.subscribe();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
        }
    }

    if shared.stopping() {
        shared.mutate(|s| s.state = State::Stopped);
    }
}

/// Run one established session until the tunnel dies or is stopped.
/// Every exit path performs a FULL teardown of the forwarder set.
async fn run_session(
    cfg: &SupervisorConfig,
    mut reader: FrameReader<OwnedReadHalf>,
    // Held (not written to) for the whole session: dropping the owned write
    // half shuts down the write side of the TCP stream, which the server
    // would read as end-of-stream and tear the tunnel down.
    _writer: OwnedWriteHalf,
    shared: &Arc<Shared>,
) -> Result<(), TunnelError> {
    let mut forwarders: JoinSet<()> = JoinSet::new();
    let mut stop_rx = shared.stop_tx.subscribe();
    let mut liveness_deadline = tokio::time::Instant::now() + cfg.ack_window;

    let result = loop {
        tokio::select! {
            // No `if` guard here: select! preconditions are evaluated
            // once per call, so a guarded arm would never wake an idle
            // session. Poll always, decide in the body.
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break Ok(());
                }
            }

            msg = reader.next_server_message() => match msg {
                Ok(None) => break Err(TunnelError::Disconnected(
                    "server closed the control connection".into(),
                )),
                Err(e) => break Err(e),
                Ok(Some(ServerMessage::Connection(id))) => {
                    spawn_forwarder(cfg, &mut forwarders, shared, id);
                }
                // Both refresh signals, either dialect: bore heartbeats
                // with the bare string, Spore may Ack or heartbeat.
                Ok(Some(ServerMessage::Ack | ServerMessage::Heartbeat)) => {
                    liveness_deadline = tokio::time::Instant::now() + cfg.ack_window;
                }
                Ok(Some(ServerMessage::Error(e))) => {
                    shared.log(LogLevel::Error, format!("server error: {e}"));
                }
                Ok(Some(other)) => {
                    shared.log(
                        LogLevel::Info,
                        format!("unexpected control message: {other:?}"),
                    );
                }
            },

            _ = tokio::time::sleep_until(liveness_deadline) => {
                break Err(TunnelError::AckTimeout);
            }

            // Only armed while forwarders exist: join_next() on an empty
            // JoinSet is immediately Ready(None), which would busy-loop.
            Some(_joined) = forwarders.join_next() => {}
        }
    };

    // Full teardown: abort every forwarder and wait until all are gone.
    forwarders.abort_all();
    while forwarders.join_next().await.is_some() {}
    result
}

fn spawn_forwarder(
    cfg: &SupervisorConfig,
    forwarders: &mut JoinSet<()>,
    shared: &Arc<Shared>,
    conn_id: String,
) {
    let cfg = cfg.clone();
    let shared = shared.clone();
    shared.log(LogLevel::Info, format!("incoming connection {conn_id}"));
    forwarders.spawn(async move {
        let _guard = ConnGuard::new(shared.active_conns.clone());

        // Local preflight first: a down service must not consume a
        // server-side slot, and surfaces as LocalServiceDown.
        let mut local = match forward::connect_local(&cfg.local_host, cfg.local_port).await {
            Ok(local) => local,
            Err(e) => {
                shared.log(LogLevel::Error, format!("{e}"));
                return;
            }
        };

        let (remote, leftover) =
            match client::open_data_connection(&cfg.tunnel, &cfg.secret, &conn_id).await {
                Ok(pair) => pair,
                Err(e) => {
                    shared.log(LogLevel::Error, format!("data channel failed: {e}"));
                    return;
                }
            };

        // Bytes the visitor pushed before the raw handoff completed.
        if !leftover.is_empty() && local.write_all(&leftover).await.is_err() {
            return;
        }

        if let Err(e) =
            forward::forward_bidirectional(remote, local, shared.counters.clone()).await
        {
            shared.log(LogLevel::Error, format!("{e}"));
        }
    });
}

/// Keeps `active_conns` accurate even when the task is aborted.
struct ConnGuard(Arc<AtomicUsize>);

impl ConnGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Exponential reconnect delay with ±20% jitter:
/// `base * 2^deaths`, capped at `max`.
pub fn backoff_delay(deaths: u32, base: Duration, max: Duration, rng: &mut Jitter) -> Duration {
    let shift = deaths.min(6);
    let nominal = base.saturating_mul(1u32 << shift).min(max);
    let jitter = 0.8 + (rng.next() % 400) as f64 / 1000.0; // [0.8, 1.2)
    nominal.mul_f64(jitter)
}

/// Tiny xorshift RNG — enough entropy for jitter, no extra dependency.
pub struct Jitter(u64);

impl Jitter {
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0x853C49E6748FEA9B);
        Self(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

impl Default for Jitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::client::TunnelConfig;
    use crate::tunnel::mock_server::{mock, spawn_reply_once_service, Dialect};
    use std::net::SocketAddr;

    fn tunnel_cfg(addr: SocketAddr) -> TunnelConfig {
        TunnelConfig {
            server: "127.0.0.1".to_string(),
            control_port: addr.port(),
            remote_port: 0,
        }
    }

    fn fast_config(control: SocketAddr, secret: &str, local_port: u16) -> SupervisorConfig {
        SupervisorConfig {
            tunnel: tunnel_cfg(control),
            secret: secret.to_string(),
            local_host: "127.0.0.1".to_string(),
            local_port,
            auto_reconnect: true,
            ack_window: Duration::from_millis(300),
            backoff_base: Duration::from_millis(30),
            backoff_max: Duration::from_millis(120),
        }
    }

    async fn eventually(
        sup: &TunnelSupervisor,
        within: Duration,
        pred: impl Fn(&TunnelStatus) -> bool,
    ) -> TunnelStatus {
        let deadline = Instant::now() + within;
        loop {
            let status = sup.status();
            if pred(&status) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "condition not met in {within:?}; last status: {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn bore_lifecycle_with_data_plane() {
        // Heartbeating bore mock: with the liveness window armed in both
        // dialects, a silent server would be declared dead.
        let mock = mock()
            .ack_interval(Duration::from_millis(50))
            .start()
            .await
            .unwrap();
        let (local, _local_handle) = spawn_reply_once_service().await.unwrap();
        let sup = TunnelSupervisor::new();
        sup.start(fast_config(mock.control_addr(), "", local.port()))
            .await
            .unwrap();

        let status = sup.wait_for(&["connected"], Duration::from_secs(3)).await;
        assert_eq!(status.state, "connected");
        assert_eq!(status.server_kind.as_deref(), Some("Bore"));
        assert_eq!(
            status.remote_address.as_deref(),
            Some(format!("127.0.0.1:{}", mock.assigned_port()).as_str())
        );
        assert_eq!(status.assigned_remote_port, Some(mock.assigned_port()));
        assert_eq!(status.uptime_secs, 0, "just connected");
        assert!(status.last_error.is_none());
        assert!(!status.logs.is_empty());

        // Visitor arrives: bridge greets (6 B down), local replies `ack\n`
        // (4 B up), bridge echoes it back (4 B down).
        mock.trigger_connection().await.unwrap();
        let status = eventually(
            &sup,
            Duration::from_secs(3),
            |s| s.bytes_up == 10 && s.bytes_down == 4,
        )
        .await;
        assert_eq!(status.reconnects, 0);
        assert_eq!(sup.active_connections(), 1);

        // Several liveness windows (300 ms) pass while the server keeps
        // heartbeating every 50 ms: the session must survive them all
        // without a single reconnect.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let status = sup.status();
        assert_eq!(status.state, "connected", "status: {status:?}");
        assert_eq!(status.reconnects, 0, "status: {status:?}");
        assert_eq!(mock.hello_count(), 1, "no silent reconnects allowed");

        sup.stop().await;
        let status = sup.status();
        assert_eq!(status.state, "stopped");
        assert_eq!(sup.active_connections(), 0, "forwarders must be gone");
        mock.stop().await;
    }

    #[tokio::test]
    async fn bore_heartbeat_silence_declares_death_and_reconnects() {
        // A bore server that stops heartbeating (dead without FIN/RST, the
        // ghost-port scenario) must be declared dead by the liveness window
        // and reconnected with a fresh legacy-Hello handshake.
        let mock = mock().start().await.unwrap(); // silent control plane
        let (local, _local_handle) = spawn_reply_once_service().await.unwrap();
        let mut cfg = fast_config(mock.control_addr(), "", local.port());
        cfg.ack_window = Duration::from_millis(120);
        let sup = TunnelSupervisor::new();
        sup.start(cfg).await.unwrap();
        sup.wait_for(&["connected"], Duration::from_secs(3)).await;

        let status = eventually(&sup, Duration::from_secs(5), |s| s.reconnects >= 2).await;
        assert_eq!(status.server_kind.as_deref(), Some("Bore"));
        assert!(matches!(status.last_error.as_deref(), Some(e) if e.contains("ack")));
        assert!(
            sup.wait_for(&["connected"], Duration::from_secs(3))
                .await
                .state
                == "connected"
        );
        assert!(mock.hello_count() >= 3, "hellos: {}", mock.hello_count());
        sup.stop().await;
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_acks_keep_the_session_alive() {
        let mock = mock()
            .dialect(Dialect::Spore)
            .ack_interval(Duration::from_millis(40))
            .start()
            .await
            .unwrap();
        let (local, _local_handle) = spawn_reply_once_service().await.unwrap();
        let mut cfg = fast_config(mock.control_addr(), "", local.port());
        cfg.ack_window = Duration::from_millis(300);
        let sup = TunnelSupervisor::new();
        sup.start(cfg).await.unwrap();
        let status = sup.wait_for(&["connected"], Duration::from_secs(3)).await;
        assert_eq!(status.server_kind.as_deref(), Some("Spore"));

        // Several ack windows pass — the session must survive all of them.
        tokio::time::sleep(Duration::from_millis(900)).await;
        let status = sup.status();
        assert_eq!(status.state, "connected", "status: {status:?}");
        assert_eq!(status.reconnects, 0);
        sup.stop().await;
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_ack_silence_declares_death_and_reconnects() {
        let mock = mock().dialect(Dialect::Spore).start().await.unwrap(); // no acks
        let (local, _local_handle) = spawn_reply_once_service().await.unwrap();
        let mut cfg = fast_config(mock.control_addr(), "", local.port());
        cfg.ack_window = Duration::from_millis(120);
        let sup = TunnelSupervisor::new();
        sup.start(cfg).await.unwrap();
        sup.wait_for(&["connected"], Duration::from_secs(3)).await;

        let status = eventually(
            &sup,
            Duration::from_secs(5),
            |s| s.reconnects >= 2,
        )
        .await;
        assert_eq!(status.server_kind.as_deref(), Some("Spore"));
        assert!(matches!(status.last_error.as_deref(), Some(e) if e.contains("ack")));
        // Every reconnect went through a fresh HelloEx handshake (assert
        // after the final session is established, not mid-handshake).
        assert!(
            sup.wait_for(&["connected"], Duration::from_secs(3))
                .await
                .state
                == "connected"
        );
        assert!(mock.hello_ex_count() >= 3, "hello_ex: {}", mock.hello_ex_count());
        sup.stop().await;
        mock.stop().await;
    }

    #[tokio::test]
    async fn control_death_tears_down_forwarders_and_reconnects() {
        let mock = mock()
            .ack_interval(Duration::from_millis(50))
            .start()
            .await
            .unwrap();
        let (local, _local_handle) = spawn_reply_once_service().await.unwrap();
        let sup = TunnelSupervisor::new();
        sup.start(fast_config(mock.control_addr(), "", local.port()))
            .await
            .unwrap();
        sup.wait_for(&["connected"], Duration::from_secs(3)).await;

        mock.trigger_connection().await.unwrap();
        mock.trigger_connection().await.unwrap();
        eventually(&sup, Duration::from_secs(3), |_| sup.active_connections() >= 2).await;

        mock.drop_control().await;
        // The death must be *observed* before we can assert the reconnect:
        // waiting for "connected" alone would match the still-live session.
        eventually(&sup, Duration::from_secs(3), |s| s.reconnects >= 1).await;
        eventually(&sup, Duration::from_secs(3), |s| s.state == "connected").await;
        // … and the old session's forwarders are fully gone.
        eventually(&sup, Duration::from_secs(3), |_| sup.active_connections() == 0).await;
        sup.stop().await;
        mock.stop().await;
    }

    #[tokio::test]
    async fn stop_during_backoff_returns_promptly() {
        let mock = mock().dialect(Dialect::Spore).start().await.unwrap();
        let (local, _local_handle) = spawn_reply_once_service().await.unwrap();
        let mut cfg = fast_config(mock.control_addr(), "", local.port());
        cfg.ack_window = Duration::from_millis(80);
        cfg.backoff_base = Duration::from_secs(30); // long sleep to interrupt
        let sup = TunnelSupervisor::new();
        sup.start(cfg).await.unwrap();
        sup.wait_for(&["connected"], Duration::from_secs(3)).await;
        eventually(&sup, Duration::from_secs(3), |s| s.reconnects >= 1).await;

        let started = Instant::now();
        sup.stop().await;
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "stop took {:?}",
            started.elapsed()
        );
        assert_eq!(sup.status().state, "stopped");
        mock.stop().await;
    }

    #[tokio::test]
    async fn failed_connect_without_auto_reconnect_is_final() {
        // Port with no listener behind it.
        let tmp = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = tmp.local_addr().unwrap();
        drop(tmp);

        let mut cfg = fast_config(addr, "", 25565);
        cfg.auto_reconnect = false;
        let sup = TunnelSupervisor::new();
        sup.start(cfg).await.unwrap();
        let status = sup.wait_for(&["failed"], Duration::from_secs(5)).await;
        assert_eq!(status.state, "failed");
        assert!(status.last_error.is_some());
        sup.stop().await;
    }

    #[test]
    fn backoff_delay_stays_within_jitter_bounds() {
        let base = Duration::from_secs(5);
        let max = Duration::from_secs(60);
        let mut rng = Jitter::new();
        for deaths in 0..8u32 {
            let nominal = base
                .saturating_mul(1u32 << deaths.min(6))
                .min(max);
            let lo = nominal.mul_f64(0.8);
            let hi = nominal.mul_f64(1.2);
            for _ in 0..100 {
                let d = backoff_delay(deaths, base, max, &mut rng);
                assert!(d >= lo && d <= hi, "deaths={deaths} delay={d:?}");
            }
        }
        // Cap applies: far deaths never exceed max * 1.2.
        let capped_hi = max.mul_f64(1.2);
        for _ in 0..100 {
            let d = backoff_delay(50, base, max, &mut rng);
            assert!(d <= capped_hi, "delay={d:?}");
        }
    }

    #[test]
    fn log_ring_is_capped_at_1024_indexed_lines() {
        let mut s = InnerState::default();
        for i in 0..1500 {
            s.push_log(LogLevel::Error, format!("line-{i}"));
        }
        assert_eq!(s.log_ring.len(), MAX_LOG_LINES);
        // Oldest survivor is exactly MAX_LOG_LINES behind the newest index.
        assert_eq!(s.log_ring.front().unwrap().index, 1500 - MAX_LOG_LINES as u64);
        assert_eq!(s.log_ring.front().unwrap().line, "line-476");
        assert_eq!(s.log_ring.back().unwrap().index, 1499);
        assert_eq!(s.log_ring.back().unwrap().line, "line-1499");
        assert_eq!(s.next_log_index, 1500);
        // Every entry carries a timestamp and its level.
        assert!(s.log_ring.iter().all(|e| e.ts > 0 && e.level == LogLevel::Error));
    }

    #[test]
    fn log_entries_since_returns_strictly_newer_entries() {
        let mut s = InnerState::default();
        s.push_log(LogLevel::Info, "a".to_string());
        s.push_log(LogLevel::Error, "b".to_string());
        s.push_log(LogLevel::Info, "c".to_string());

        let tail = s.log_ring.iter().filter(|e| e.index > 0).cloned().collect::<Vec<_>>();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].line, "b");
        assert_eq!(tail[0].level, LogLevel::Error);
        assert_eq!(tail[1].line, "c");

        // Nothing newer than the newest index.
        assert_eq!(s.log_ring.iter().filter(|e| e.index > 2).count(), 0);
    }

    #[tokio::test]
    async fn supervisor_exposes_log_lines_without_prefixes() {
        let sup = TunnelSupervisor::new();
        // Port with no listener behind it: deterministic failure logs.
        let tmp = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = tmp.local_addr().unwrap();
        drop(tmp);

        let mut cfg = fast_config(addr, "", 25565);
        cfg.auto_reconnect = false;
        sup.start(cfg).await.unwrap();
        let status = sup.wait_for(&["failed"], Duration::from_secs(5)).await;

        assert!(status
            .logs
            .iter()
            .any(|l| l.starts_with("connect failed:") || l.starts_with("connecting to")));
        // The status snapshot may race one late log line; compare via a
        // fresh full snapshot and assert the incremental read agrees.
        let entries = sup.log_snapshot();
        assert!(entries.len() >= status.logs.len());
        assert!(entries.iter().any(|e| e.level == LogLevel::Error && e.ts > 0));
        let last = sup.last_log_index();
        assert!(last > 0);
        // Strictly-newer semantics: `since = last` yields nothing …
        assert!(sup.log_entries_since(last).is_empty());
        // … and a tail read skips exactly what was already seen.
        let tail = sup.log_entries_since(last - 1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].index, last);
        sup.stop().await;
    }
}

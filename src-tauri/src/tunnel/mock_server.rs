//! In-process mock tunnel server for tests.
//!
//! Simulates both server dialects the client must handle:
//!
//! * [`Dialect::Bore`] — answers a legacy `Hello(port)` with `{"Hello":p}`;
//!   a strict Rust `bore` server cannot deserialize `HelloEx` and just
//!   drops the connection ([`MockServerBuilder::drop_on_hello_ex`]).
//! * [`Dialect::Spore`] — a strict Spore server drops legacy `Hello`
//!   (forcing the client's HelloEx retry) and answers `HelloEx` with the
//!   extended port/features payload. Challenges are sent as bare strings.
//!
//! The data plane is a loopback echo listener bound on `assigned_port`
//! (0 = ephemeral; [`MockServer::assigned_port`] reports the real port).
//! Incoming `Connection(id)` notifications are fired on demand via
//! [`MockServer::trigger_connection`]; the client's `Accept(id)` data
//! connections are bridged to a greeting+echo pump.

use super::protocol::{
    challenge_answer, encode_frame, parse_client_message, parse_server_message, send,
    ClientMessage, FrameDecoder, FrameReader, ServerMessage,
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

/// Secret the mock expects when [`MockServerBuilder::require_auth`] is on.
pub const DEFAULT_SECRET: &str = "test-secret";

/// Wire dialect spoken by the mock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Answers legacy `Hello`; drops the connection on `HelloEx`
    /// (like the Rust bore server, which cannot decode it).
    Bore,
    /// Strict Spore server: drops legacy `Hello`, answers `HelloEx`.
    Spore,
}

enum ConnCmd {
    SendConnection(String),
    Close,
}

struct Shared {
    dialect: Dialect,
    secret: String,
    require_auth: bool,
    ack_interval: Option<Duration>,
    drop_on_hello_ex: bool,
    assigned_port: AtomicU16,
    hellos: AtomicUsize,
    hello_exes: AtomicUsize,
    accepts: AtomicUsize,
    auth_failures: AtomicUsize,
    authenticated: AtomicBool,
    control_tx: Mutex<Option<mpsc::UnboundedSender<ConnCmd>>>,
    stop_tx: watch::Sender<bool>,
}

/// Handle to a running mock server.
pub struct MockServer {
    shared: Arc<Shared>,
    control_addr: SocketAddr,
    echo_addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

/// Builder for [`MockServer`].
pub struct MockServerBuilder {
    dialect: Dialect,
    require_auth: bool,
    ack_interval: Option<Duration>,
    drop_on_hello_ex: bool,
    assigned_port: u16,
    secret: String,
}

/// Start building a mock server (Bore dialect, no auth, no acks by default).
pub fn mock() -> MockServerBuilder {
    MockServerBuilder {
        dialect: Dialect::Bore,
        require_auth: false,
        ack_interval: None,
        drop_on_hello_ex: false,
        assigned_port: 0,
        secret: DEFAULT_SECRET.to_string(),
    }
}

impl MockServerBuilder {
    pub fn dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn require_auth(mut self, require: bool) -> Self {
        self.require_auth = require;
        self
    }

    pub fn ack_interval(mut self, interval: Duration) -> Self {
        self.ack_interval = Some(interval);
        self
    }

    /// Reproduces the Rust bore server, which drops the connection upon
    /// receiving an undecodable `HelloEx`.
    pub fn drop_on_hello_ex(mut self, drop: bool) -> Self {
        self.drop_on_hello_ex = drop;
        self
    }

    /// Port for the data-plane echo listener (0 = ephemeral).
    pub fn assigned_port(mut self, port: u16) -> Self {
        self.assigned_port = port;
        self
    }

    pub fn secret(mut self, secret: &str) -> Self {
        self.secret = secret.to_string();
        self
    }

    pub async fn start(self) -> std::io::Result<MockServer> {
        let echo = TcpListener::bind(("127.0.0.1", self.assigned_port)).await?;
        let control = TcpListener::bind(("127.0.0.1", 0)).await?;
        let control_addr = control.local_addr()?;
        let echo_addr = echo.local_addr()?;
        let (stop_tx, stop_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            dialect: self.dialect,
            secret: self.secret,
            require_auth: self.require_auth,
            ack_interval: self.ack_interval,
            drop_on_hello_ex: self.drop_on_hello_ex,
            assigned_port: AtomicU16::new(echo_addr.port()),
            hellos: AtomicUsize::new(0),
            hello_exes: AtomicUsize::new(0),
            accepts: AtomicUsize::new(0),
            auth_failures: AtomicUsize::new(0),
            authenticated: AtomicBool::new(false),
            control_tx: Mutex::new(None),
            stop_tx,
        });
        let task = tokio::spawn(run_listeners(control, echo, shared.clone(), stop_rx));
        Ok(MockServer {
            shared,
            control_addr,
            echo_addr,
            task,
        })
    }
}

impl MockServer {
    /// Control-channel address (where the client connects).
    pub fn control_addr(&self) -> SocketAddr {
        self.control_addr
    }

    /// Data-plane echo listener address (also the port assigned in handshakes).
    pub fn echo_addr(&self) -> SocketAddr {
        self.echo_addr
    }

    /// Port the mock hands back as the tunnel's assigned remote port
    /// (changes after [`MockServer::set_assigned_port`]; the echo
    /// listener stays on the port it was bound to).
    pub fn assigned_port(&self) -> u16 {
        self.shared.assigned_port.load(Ordering::Relaxed)
    }

    /// Change the port advertised in future handshakes — used with
    /// [`MockServer::drop_control`] to simulate a server restart that
    /// reassigns tunnels to different ports.
    pub fn set_assigned_port(&self, port: u16) {
        self.shared.assigned_port.store(port, Ordering::Relaxed);
    }

    pub fn hello_count(&self) -> usize {
        self.shared.hellos.load(Ordering::Relaxed)
    }

    pub fn hello_ex_count(&self) -> usize {
        self.shared.hello_exes.load(Ordering::Relaxed)
    }

    pub fn accept_count(&self) -> usize {
        self.shared.accepts.load(Ordering::Relaxed)
    }

    pub fn auth_failure_count(&self) -> usize {
        self.shared.auth_failures.load(Ordering::Relaxed)
    }

    pub fn authenticated(&self) -> bool {
        self.shared.authenticated.load(Ordering::Relaxed)
    }

    /// Fire a `Connection(id)` notification on the established control
    /// connection, as a real server would when a visitor arrives.
    pub async fn trigger_connection(&self) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let tx = self.shared.control_tx.lock().unwrap().clone();
        match tx {
            Some(tx) => {
                tx.send(ConnCmd::SendConnection(id.clone()))
                    .map_err(|e| e.to_string())?;
                Ok(id)
            }
            None => Err("no established control connection".to_string()),
        }
    }

    /// Abruptly close the current control connection (simulates server death).
    pub async fn drop_control(&self) {
        if let Some(tx) = self.shared.control_tx.lock().unwrap().clone() {
            let _ = tx.send(ConnCmd::Close);
        }
    }

    /// Shut the whole mock down (listeners and every connection).
    pub async fn stop(self) {
        let _ = self.shared.stop_tx.send(true);
        self.task.abort();
    }
}

async fn run_listeners(
    control: TcpListener,
    echo: TcpListener,
    shared: Arc<Shared>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
            accepted = control.accept() => {
                if let Ok((stream, _)) = accepted {
                    let shared = shared.clone();
                    let stop_rx = stop_rx.clone();
                    conns.spawn(handle_conn(stream, shared, stop_rx));
                }
            }
            accepted = echo.accept() => {
                if let Ok((stream, _)) = accepted {
                    conns.spawn(echo_pump(stream));
                }
            }
            // Only armed while tasks exist: join_next() on an empty
            // JoinSet is immediately Ready(None), which would busy-loop.
            Some(_) = conns.join_next() => {}
        }
    }
    conns.abort_all();
}

async fn handle_conn(
    stream: TcpStream,
    shared: Arc<Shared>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let (rh, mut wh) = stream.into_split();
    let mut reader = FrameReader::new(rh);

    // Auth phase: challenge immediately, expect the HMAC answer.
    if shared.require_auth {
        let nonce = uuid::Uuid::new_v4().to_string();
        let challenge: serde_json::Value = match shared.dialect {
            Dialect::Bore => serde_json::to_value(ServerMessage::Challenge(nonce.clone()))
                .expect("encode challenge"),
            // Spore servers send the nonce as a bare JSON string.
            Dialect::Spore => serde_json::Value::String(nonce.clone()),
        };
        if send(&mut wh, &challenge).await.is_err() {
            return;
        }
        match read_client(&mut reader, Duration::from_secs(5)).await {
            Some(ClientMessage::Authenticate(answer))
                if answer == challenge_answer(&shared.secret, &nonce) =>
            {
                shared.authenticated.store(true, Ordering::Relaxed);
            }
            Some(ClientMessage::Authenticate(_)) => {
                shared.auth_failures.fetch_add(1, Ordering::Relaxed);
                let _ = send(&mut wh, &ServerMessage::Error("invalid secret".into())).await;
                return;
            }
            _ => {
                shared.auth_failures.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    // Hello / Accept phase.
    match read_client(&mut reader, Duration::from_secs(5)).await {
        Some(ClientMessage::Hello(_)) => {
            shared.hellos.fetch_add(1, Ordering::Relaxed);
            match shared.dialect {
                Dialect::Bore => {
                    if send(
                        &mut wh,
                        &ServerMessage::Hello(shared.assigned_port.load(Ordering::Relaxed)),
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
                // Strict Spore server: cannot deal with a legacy Hello.
                Dialect::Spore => return,
            }
        }
        Some(ClientMessage::HelloEx { .. }) => {
            shared.hello_exes.fetch_add(1, Ordering::Relaxed);
            if shared.drop_on_hello_ex {
                return; // reproduce the Rust bore server: abrupt drop.
            }
            match shared.dialect {
                Dialect::Bore => {
                    let _ = send(
                        &mut wh,
                        &ServerMessage::Error("unknown message HelloEx".into()),
                    )
                    .await;
                    return;
                }
                Dialect::Spore => {
                    let reply = ServerMessage::HelloEx {
                        port: shared.assigned_port.load(Ordering::Relaxed),
                        features: vec!["ack".to_string()],
                    };
                    if send(&mut wh, &reply).await.is_err() {
                        return;
                    }
                }
            }
        }
        Some(ClientMessage::Accept(_id)) => {
            shared.accepts.fetch_add(1, Ordering::Relaxed);
            let (rh, mut decoder) = reader.into_parts();
            let leftover = decoder.drain_buffer();
            let mut stream = rh
                .reunite(wh)
                .expect("read and write half of the same TcpStream");
            visitor_bridge(&mut stream, leftover).await;
            return;
        }
        _ => return,
    }

    // Established control loop: pump Acks, relay on-demand Connection frames.
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    *shared.control_tx.lock().unwrap() = Some(cmd_tx);
    let send_acks = shared.ack_interval.is_some();
    let mut ticker =
        tokio::time::interval(shared.ack_interval.unwrap_or(Duration::from_secs(3600)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
            frame = reader.next_frame() => match frame {
                // Heartbeats and anything else the client sends: ignored.
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            },
            cmd = cmd_rx.recv() => match cmd {
                Some(ConnCmd::SendConnection(id)) => {
                    if send(&mut wh, &ServerMessage::Connection(id)).await.is_err() {
                        break;
                    }
                }
                Some(ConnCmd::Close) | None => break,
            },
            _ = ticker.tick() => {
                if send_acks && send(&mut wh, &ServerMessage::Ack).await.is_err() {
                    break;
                }
            }
        }
    }
    *shared.control_tx.lock().unwrap() = None;
}

/// Bytes a visitor would push through the tunnel: greet, echo early
/// client bytes that arrived together with `Accept`, then echo forever.
async fn visitor_bridge(stream: &mut TcpStream, leftover: Vec<u8>) {
    let _ = stream.write_all(b"hello\n").await;
    if !leftover.is_empty() {
        let _ = stream.write_all(&leftover).await;
    }
    let mut buf = vec![0u8; 4096];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Pure echo pump for the data-plane listener on `assigned_port`.
async fn echo_pump(mut stream: TcpStream) {
    let mut buf = vec![0u8; 4096];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Read one client message with a deadline (None on EOF/timeout/garbage).
async fn read_client(
    reader: &mut FrameReader<OwnedReadHalf>,
    deadline: Duration,
) -> Option<ClientMessage> {
    let frame = match timeout(deadline, reader.next_frame()).await {
        Ok(Ok(frame)) => frame?,
        _ => return None,
    };
    parse_client_message(&frame).ok()
}

/// Loopback TCP service for forwarder/supervisor tests: replies `ack\n` to
/// the FIRST line it receives, then keeps reading silently. This makes byte
/// counters deterministic (no echo ping-pong loops).
pub async fn spawn_reply_once_service()
-> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1024];
                let mut acc: Vec<u8> = Vec::new();
                let mut replied = false;
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            acc.extend_from_slice(&buf[..n]);
                            if !replied && acc.contains(&b'\n') {
                                replied = true;
                                let _ = sock.write_all(b"ack\n").await;
                                acc.clear();
                            }
                        }
                    }
                }
            });
        }
    });
    Ok((addr, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    const HANDSHAKE_WAIT: Duration = Duration::from_secs(2);

    /// Minimal scripted client: raw socket + protocol helpers.
    struct RawClient {
        stream: TcpStream,
        decoder: FrameDecoder,
    }

    impl RawClient {
        async fn connect(addr: SocketAddr) -> Self {
            Self {
                stream: TcpStream::connect(addr).await.unwrap(),
                decoder: FrameDecoder::new(),
            }
        }

        async fn send(&mut self, msg: &ClientMessage) {
            let frame = encode_frame(msg).unwrap();
            self.stream.write_all(&frame).await.unwrap();
        }

        /// Read one framed server message; None means EOF/timeout.
        async fn recv(&mut self) -> Option<ServerMessage> {
            let mut chunk = [0u8; 4096];
            loop {
                if let Ok(Some(payload)) = self.decoder.next_payload() {
                    return parse_server_message(&payload).ok();
                }
                let n = timeout(HANDSHAKE_WAIT, self.stream.read(&mut chunk))
                    .await
                    .ok()?
                    .ok()?;
                if n == 0 {
                    return None;
                }
                self.decoder.push(&chunk[..n]);
            }
        }

        /// Read raw (unframed) bytes — data-plane mode after `Accept`.
        async fn recv_raw(&mut self, want: usize) -> Option<Vec<u8>> {
            let mut out = Vec::with_capacity(want);
            let mut chunk = [0u8; 4096];
            while out.len() < want {
                let n = timeout(HANDSHAKE_WAIT, self.stream.read(&mut chunk))
                    .await
                    .ok()?
                    .ok()?;
                if n == 0 {
                    return None;
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Some(out)
        }
    }

    #[tokio::test]
    async fn bore_handshake_answers_legacy_hello() {
        let mock = mock().start().await.unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        client.send(&ClientMessage::Hello(0)).await;
        match client.recv().await {
            Some(ServerMessage::Hello(port)) => assert_eq!(port, mock.assigned_port()),
            other => panic!("expected Hello, got {other:?}"),
        }
        assert_eq!(mock.hello_count(), 1);
        assert_eq!(mock.hello_ex_count(), 0);
        mock.stop().await;
    }

    #[tokio::test]
    async fn bore_handshake_with_auth() {
        let mock = mock().require_auth(true).start().await.unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        match client.recv().await {
            Some(ServerMessage::Challenge(nonce)) => {
                let answer = challenge_answer(DEFAULT_SECRET, &nonce);
                client.send(&ClientMessage::Authenticate(answer)).await;
            }
            other => panic!("expected Challenge, got {other:?}"),
        }
        client.send(&ClientMessage::Hello(0)).await;
        assert!(matches!(client.recv().await, Some(ServerMessage::Hello(_))));
        assert!(mock.authenticated());
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_sends_bare_string_challenge() {
        let mock = mock()
            .dialect(Dialect::Spore)
            .require_auth(true)
            .start()
            .await
            .unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        // Spore challenge arrives as a bare JSON string; the decoder must
        // still recognize it as a Challenge.
        match client.recv().await {
            Some(ServerMessage::Challenge(nonce)) => {
                assert!(!nonce.is_empty());
                let answer = challenge_answer(DEFAULT_SECRET, &nonce);
                client.send(&ClientMessage::Authenticate(answer)).await;
            }
            other => panic!("expected Challenge, got {other:?}"),
        }
        client
            .send(&ClientMessage::HelloEx {
                port: 0,
                version: "spore/1".into(),
                features: vec![],
            })
            .await;
        match client.recv().await {
            Some(ServerMessage::HelloEx { port, features }) => {
                assert_eq!(port, mock.assigned_port());
                assert!(features.contains(&"ack".to_string()));
            }
            other => panic!("expected HelloEx, got {other:?}"),
        }
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_drops_legacy_hello() {
        let mock = mock().dialect(Dialect::Spore).start().await.unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        client.send(&ClientMessage::Hello(0)).await;
        assert!(client.recv().await.is_none(), "connection must be dropped");
        assert_eq!(mock.hello_count(), 1);
        assert_eq!(mock.hello_ex_count(), 0);
        mock.stop().await;
    }

    #[tokio::test]
    async fn drop_on_hello_ex_reproduces_bore_server() {
        let mock = mock()
            .dialect(Dialect::Bore)
            .drop_on_hello_ex(true)
            .start()
            .await
            .unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        client
            .send(&ClientMessage::HelloEx {
                port: 0,
                version: "spore/1".into(),
                features: vec![],
            })
            .await;
        assert!(client.recv().await.is_none(), "connection must be dropped");
        assert_eq!(mock.hello_ex_count(), 1);
        mock.stop().await;
    }

    #[tokio::test]
    async fn wrong_secret_gets_error_frame() {
        let mock = mock().require_auth(true).start().await.unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        match client.recv().await {
            Some(ServerMessage::Challenge(_)) => {
                client
                    .send(&ClientMessage::Authenticate("deadbeef".into()))
                    .await;
            }
            other => panic!("expected Challenge, got {other:?}"),
        }
        assert!(matches!(
            client.recv().await,
            Some(ServerMessage::Error(ref e)) if e.contains("secret")
        ));
        assert_eq!(mock.auth_failure_count(), 1);
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_sends_ack_frames_periodically() {
        let mock = mock()
            .dialect(Dialect::Spore)
            .ack_interval(Duration::from_millis(30))
            .start()
            .await
            .unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        client
            .send(&ClientMessage::HelloEx {
                port: 0,
                version: "spore/1".into(),
                features: vec![],
            })
            .await;
        assert!(matches!(
            client.recv().await,
            Some(ServerMessage::HelloEx { .. })
        ));
        assert!(matches!(client.recv().await, Some(ServerMessage::Ack)));
        assert!(matches!(client.recv().await, Some(ServerMessage::Ack)));
        mock.stop().await;
    }

    #[tokio::test]
    async fn echo_listener_reflects_bytes() {
        let mock = mock().start().await.unwrap();
        let mut sock = TcpStream::connect(mock.echo_addr()).await.unwrap();
        sock.write_all(b"abc").await.unwrap();
        let mut buf = [0u8; 8];
        let n = timeout(HANDSHAKE_WAIT, sock.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"abc");
        mock.stop().await;
    }

    #[tokio::test]
    async fn trigger_connection_and_accept_bridge() {
        let mock = mock().dialect(Dialect::Spore).start().await.unwrap();
        let mut control = RawClient::connect(mock.control_addr()).await;
        control
            .send(&ClientMessage::HelloEx {
                port: 0,
                version: "spore/1".into(),
                features: vec![],
            })
            .await;
        assert!(matches!(
            control.recv().await,
            Some(ServerMessage::HelloEx { .. })
        ));

        // Server announces a visitor; the client dials a data connection
        // and pipelines a payload right after the Accept frame.
        mock.trigger_connection().await.unwrap();
        let conn_id = match control.recv().await {
            Some(ServerMessage::Connection(id)) => id,
            other => panic!("expected Connection, got {other:?}"),
        };

        let mut data = RawClient::connect(mock.control_addr()).await;
        let accept_frame = encode_frame(&ClientMessage::Accept(conn_id)).unwrap();
        let mut batch = accept_frame.clone();
        batch.extend_from_slice(b"ping\n");
        data.stream.write_all(&batch).await.unwrap();

        // Bridge greets like a visitor and echoes the pipelined bytes.
        let got = data.recv_raw("hello\nping\n".len()).await.unwrap();
        assert_eq!(got, b"hello\nping\n");
        assert_eq!(mock.accept_count(), 1);
        mock.stop().await;
    }

    #[tokio::test]
    async fn drop_control_closes_the_control_connection() {
        let mock = mock().start().await.unwrap();
        let mut client = RawClient::connect(mock.control_addr()).await;
        client.send(&ClientMessage::Hello(0)).await;
        assert!(matches!(client.recv().await, Some(ServerMessage::Hello(_))));
        mock.drop_control().await;
        assert!(client.recv().await.is_none(), "connection must be closed");
        mock.stop().await;
    }
}

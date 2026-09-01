//! Control-channel client.
//!
//! Handshake strategy (dialect negotiation): always send the legacy
//! `Hello(port)` first for bore compatibility; if the connection is
//! dropped or the reply is undecodable — a strict Spore server drops
//! what it cannot speak — reconnect ONCE with
//! `HelloEx { version: "spore/1", .. }`. An explicit server `Error` is
//! final (no retry). If a secret is configured, the server's `Challenge`
//! is answered with the bore-compatible HMAC-SHA256 response before the
//! Hello exchange.

use super::protocol::{
    challenge_answer, send, ClientMessage, FrameReader, ServerMessage, CLIENT_VERSION,
};
use super::TunnelError;
use std::time::Duration;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// TCP connect timeout for control and data connections.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long to wait for an unsolicited server `Challenge` before deciding
/// the server does not require authentication.
const AUTH_WAIT: Duration = Duration::from_secs(1);
/// How long to wait for each handshake reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// Which server implementation answered the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
    Bore,
    Spore,
}

impl ServerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bore => "Bore",
            Self::Spore => "Spore",
        }
    }
}

/// Capabilities reported by a Spore server alongside its HelloEx reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub kind: ServerKind,
    pub features: Vec<String>,
}

/// Where and how to reach the tunnel server.
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub server: String,
    pub control_port: u16,
    /// Desired remote port (0 = let the server assign one).
    pub remote_port: u16,
}

impl TunnelConfig {
    pub fn control_addr(&self) -> String {
        format!("{}:{}", self.server, self.control_port)
    }
}

/// An established control connection.
#[derive(Debug)]
pub struct TunnelConnection {
    assigned_port: u16,
    server_info: ServerInfo,
    reader: FrameReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl TunnelConnection {
    /// Remote port the server assigned to this tunnel.
    pub fn assigned_port(&self) -> u16 {
        self.assigned_port
    }

    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    /// Next inbound control message; `Ok(None)` on clean EOF.
    pub async fn next_message(&mut self) -> Result<Option<ServerMessage>, TunnelError> {
        self.reader.next_server_message().await
    }

    pub async fn send(&mut self, msg: &ClientMessage) -> Result<(), TunnelError> {
        send(&mut self.writer, msg).await?;
        Ok(())
    }

    /// Split into the negotiated info plus raw stream halves (used by the
    /// supervisor, which must read and write concurrently).
    pub fn into_parts(
        self,
    ) -> (u16, ServerInfo, FrameReader<OwnedReadHalf>, OwnedWriteHalf) {
        (self.assigned_port, self.server_info, self.reader, self.writer)
    }
}

/// Namespace type for the client API.
pub struct TunnelClient;

impl TunnelClient {
    /// Connect to `cfg`, authenticating with `secret` when the server asks.
    ///
    /// Returns the established control connection, or the final error after
    /// at most one legacy→HelloEx retry.
    pub async fn connect(cfg: &TunnelConfig, secret: &str) -> Result<TunnelConnection, TunnelError> {
        match attempt(cfg, secret, HelloKind::Legacy).await {
            Ok(conn) => Ok(conn),
            Err(fail) if fail.retry => attempt(cfg, secret, HelloKind::Ex)
                .await
                .map_err(|f| f.error),
            Err(fail) => Err(fail.error),
        }
    }
}

enum HelloKind {
    Legacy,
    Ex,
}

struct Fail {
    error: TunnelError,
    retry: bool,
}

impl From<TunnelError> for Fail {
    fn from(error: TunnelError) -> Self {
        Fail { error, retry: false }
    }
}

fn retryable(error: impl Into<TunnelError>) -> Fail {
    Fail {
        error: error.into(),
        retry: true,
    }
}

async fn attempt(
    cfg: &TunnelConfig,
    secret: &str,
    kind: HelloKind,
) -> Result<TunnelConnection, Fail> {
    // TCP connect failures are final: a retry with a different Hello
    // cannot fix an unreachable server.
    let stream = tcp_connect(cfg).await.map_err(Fail::from)?;
    let (rh, mut writer) = stream.into_split();
    let mut reader = FrameReader::new(rh);

    // Optional auth window: a server that requires authentication sends
    // its Challenge immediately; bore servers without auth send nothing.
    if !secret.is_empty() {
        match wait_for_challenge(&mut reader, &mut writer, secret).await? {
            ChallengeOutcome::Answered => {}
            ChallengeOutcome::Rejected(msg) => {
                return Err(TunnelError::ServerRejected(msg).into())
            }
            ChallengeOutcome::None => {} // server does not require auth
        }
    }

    let hello = match kind {
        HelloKind::Legacy => ClientMessage::Hello(cfg.remote_port),
        HelloKind::Ex => ClientMessage::HelloEx {
            port: cfg.remote_port,
            version: CLIENT_VERSION.to_string(),
            features: vec![],
        },
    };
    send(&mut writer, &hello)
        .await
        .map_err(|e| Fail::from(TunnelError::Io(e)))?;

    // Read the assignment; tolerate a late Challenge exactly once.
    let mut answered_late_challenge = secret.is_empty();
    loop {
        let payload = timeout(REPLY_TIMEOUT, reader.next_frame())
            .await
            .map_err(|_| retryable(TunnelError::Disconnected("handshake reply timed out".into())))?
            .map_err(|e| retryable(TunnelError::Io(e)))?;
        let payload = match payload {
            Some(p) => p,
            // Connection dropped without a reply: the classic sign of a
            // server that could not decode our Hello — retry with HelloEx.
            None => {
                return Err(retryable(TunnelError::Disconnected(
                    "server dropped the connection during the handshake".into(),
                )))
            }
        };
        let msg: ServerMessage = super::protocol::parse_server_message(&payload)
            .map_err(|e| retryable(TunnelError::Protocol(e.to_string())))?;
        match msg {
            ServerMessage::Hello(port) => {
                return Ok(TunnelConnection {
                    assigned_port: port,
                    server_info: ServerInfo {
                        kind: ServerKind::Bore,
                        features: vec![],
                    },
                    reader,
                    writer,
                })
            }
            ServerMessage::HelloEx { port, features } => {
                return Ok(TunnelConnection {
                    assigned_port: port,
                    server_info: ServerInfo {
                        kind: ServerKind::Spore,
                        features,
                    },
                    reader,
                    writer,
                })
            }
            ServerMessage::Challenge(nonce) if !answered_late_challenge => {
                answered_late_challenge = true;
                let answer = challenge_answer(secret, &nonce);
                send(&mut writer, &ClientMessage::Authenticate(answer))
                    .await
                    .map_err(|e| retryable(TunnelError::Io(e)))?;
            }
            ServerMessage::Challenge(_) => {
                return Err(TunnelError::AuthFailed(
                    "server requires authentication but no secret was provided".into(),
                )
                .into())
            }
            ServerMessage::Error(msg) => {
                return Err(TunnelError::ServerRejected(msg).into())
            }
            // Stray keepalives during the handshake: skip.
            ServerMessage::Ack | ServerMessage::Heartbeat => {}
            ServerMessage::Connection(id) => {
                return Err(retryable(TunnelError::Protocol(format!(
                    "unexpected Connection({id}) during handshake"
                ))))
            }
        }
    }
}

enum ChallengeOutcome {
    /// Challenge received and answered.
    Answered,
    /// Server replied with an error before Hello (e.g. bad secret).
    Rejected(String),
    /// No challenge within the window: server does not require auth.
    None,
}

async fn wait_for_challenge(
    reader: &mut FrameReader<OwnedReadHalf>,
    writer: &mut OwnedWriteHalf,
    secret: &str,
) -> Result<ChallengeOutcome, Fail> {
    let payload = match timeout(AUTH_WAIT, reader.next_frame()).await {
        Err(_) => return Ok(ChallengeOutcome::None), // no challenge in time
        Ok(Err(e)) => return Err(retryable(TunnelError::Io(e))),
        Ok(Ok(None)) => {
            return Err(retryable(TunnelError::Disconnected(
                "server closed the connection before the handshake".into(),
            )))
        }
        Ok(Ok(Some(payload))) => payload,
    };
    let msg = super::protocol::parse_server_message(&payload)
        .map_err(|e| retryable(TunnelError::Protocol(e.to_string())))?;
    match msg {
        ServerMessage::Challenge(nonce) => {
            let answer = challenge_answer(secret, &nonce);
            send(writer, &ClientMessage::Authenticate(answer))
                .await
                .map_err(|e| retryable(TunnelError::Io(e)))?;
            Ok(ChallengeOutcome::Answered)
        }
        ServerMessage::Error(msg) => Ok(ChallengeOutcome::Rejected(msg)),
        other => Err(retryable(TunnelError::Protocol(format!(
            "expected Challenge, got {other:?}"
        )))),
    }
}

/// Dial the server for an incoming visitor and claim it with `Accept(id)`.
///
/// Returns the raw data socket plus any payload bytes the server pushed
/// before the switch to raw mode (leftovers in the frame decoder).
pub async fn open_data_connection(
    cfg: &TunnelConfig,
    secret: &str,
    conn_id: &str,
) -> Result<(TcpStream, Vec<u8>), TunnelError> {
    let stream = tcp_connect(cfg).await?;
    let (rh, mut writer) = stream.into_split();
    let mut reader = FrameReader::new(rh);

    if !secret.is_empty() {
        match wait_for_challenge(&mut reader, &mut writer, secret).await {
            Ok(ChallengeOutcome::Answered) | Ok(ChallengeOutcome::None) => {}
            Ok(ChallengeOutcome::Rejected(msg)) => return Err(TunnelError::ServerRejected(msg)),
            Err(fail) => return Err(fail.error),
        }
    }

    send(
        &mut writer,
        &ClientMessage::Accept(conn_id.to_string()),
    )
    .await?;

    let (rh, mut decoder) = reader.into_parts();
    let stream = rh
        .reunite(writer)
        .expect("read and write half of the same TcpStream");
    Ok((stream, decoder.drain_buffer()))
}

async fn tcp_connect(cfg: &TunnelConfig) -> Result<TcpStream, TunnelError> {
    let addr = cfg.control_addr();
    let connect = TcpStream::connect(&addr);
    match timeout(CONNECT_TIMEOUT, connect).await {
        Ok(res) => res.map_err(TunnelError::Io),
        Err(_) => Err(TunnelError::ConnectTimeout { addr }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::mock_server::{mock, Dialect, DEFAULT_SECRET};
    use tokio::io::AsyncReadExt;

    fn cfg_for(addr: std::net::SocketAddr) -> TunnelConfig {
        TunnelConfig {
            server: addr.ip().to_string(),
            control_port: addr.port(),
            remote_port: 0,
        }
    }

    #[tokio::test]
    async fn bore_server_answers_on_first_hello() {
        let mock = mock().start().await.unwrap();
        let cfg = cfg_for(mock.control_addr());
        let conn = TunnelClient::connect(&cfg, "").await.unwrap();
        assert_eq!(conn.assigned_port(), mock.assigned_port());
        assert_eq!(conn.server_info().kind, ServerKind::Bore);
        assert!(conn.server_info().features.is_empty());
        assert_eq!(mock.hello_count(), 1);
        assert_eq!(mock.hello_ex_count(), 0);
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_server_negotiated_via_single_hello_ex_retry() {
        let mock = mock().dialect(Dialect::Spore).start().await.unwrap();
        let cfg = cfg_for(mock.control_addr());
        let conn = TunnelClient::connect(&cfg, "").await.unwrap();
        assert_eq!(conn.assigned_port(), mock.assigned_port());
        assert_eq!(conn.server_info().kind, ServerKind::Spore);
        assert!(conn.server_info().features.contains(&"ack".to_string()));
        // Legacy Hello tried first, dropped, then exactly one HelloEx retry.
        assert_eq!(mock.hello_count(), 1);
        assert_eq!(mock.hello_ex_count(), 1);
        mock.stop().await;
    }

    #[tokio::test]
    async fn authenticated_bore_handshake() {
        let mock = mock().require_auth(true).start().await.unwrap();
        let cfg = cfg_for(mock.control_addr());
        let conn = TunnelClient::connect(&cfg, DEFAULT_SECRET).await.unwrap();
        assert_eq!(conn.server_info().kind, ServerKind::Bore);
        assert!(mock.authenticated());
        mock.stop().await;
    }

    #[tokio::test]
    async fn wrong_secret_is_rejected_without_retry() {
        let mock = mock().require_auth(true).start().await.unwrap();
        let cfg = cfg_for(mock.control_addr());
        let err = TunnelClient::connect(&cfg, "wrong-secret").await.unwrap_err();
        assert!(matches!(err, TunnelError::ServerRejected(ref m) if m.contains("secret")));
        // Rejection is final: no HelloEx fallback was attempted.
        assert_eq!(mock.hello_count(), 0);
        assert_eq!(mock.hello_ex_count(), 0);
        mock.stop().await;
    }

    #[tokio::test]
    async fn spore_dropping_hello_ex_fails_after_exactly_one_retry() {
        let mock = mock()
            .dialect(Dialect::Spore)
            .drop_on_hello_ex(true)
            .start()
            .await
            .unwrap();
        let cfg = cfg_for(mock.control_addr());
        let err = TunnelClient::connect(&cfg, "").await.unwrap_err();
        assert!(matches!(err, TunnelError::Disconnected(_)), "got {err:?}");
        // First attempt (legacy Hello, dropped) + one HelloEx retry (dropped).
        assert_eq!(mock.hello_count(), 1);
        assert_eq!(mock.hello_ex_count(), 1);
        mock.stop().await;
    }

    #[tokio::test]
    async fn connection_refused_is_final() {
        // Grab a port and drop the listener so connect() is refused.
        let tmp = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = tmp.local_addr().unwrap();
        drop(tmp);
        let cfg = cfg_for(addr);
        let err = TunnelClient::connect(&cfg, "").await.unwrap_err();
        assert!(matches!(err, TunnelError::Io(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn open_data_connection_claims_the_visitor() {
        let mock = mock().dialect(Dialect::Spore).start().await.unwrap();
        let cfg = cfg_for(mock.control_addr());
        let mut conn = TunnelClient::connect(&cfg, "").await.unwrap();

        mock.trigger_connection().await.unwrap();
        match conn.next_message().await.unwrap() {
            Some(ServerMessage::Connection(id)) => {
                let (mut stream, leftover) = open_data_connection(&cfg, "", &id)
                    .await
                    .unwrap();
                // The mock's visitor bridge greets immediately; the greeting
                // may already be sitting in the decoder leftovers.
                let mut got = leftover;
                let mut buf = [0u8; 64];
                while got.len() < b"hello\n".len() {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0, "bridge closed before greeting");
                    got.extend_from_slice(&buf[..n]);
                }
                assert!(got.starts_with(b"hello\n"), "got {got:?}");
                assert_eq!(mock.accept_count(), 1);
            }
            other => panic!("expected Connection, got {other:?}"),
        }
        mock.stop().await;
    }
}

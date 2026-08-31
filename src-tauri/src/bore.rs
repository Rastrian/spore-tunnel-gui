use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

type HmacSha256 = Hmac<Sha256>;

// -- Wire protocol: externally-tagged JSON enums, null-delimited --

#[derive(Serialize)]
enum ClientMsg {
    Authenticate(String),
    Hello(u16),
    Accept(String),
}

#[derive(Deserialize, Debug)]
enum ServerMsg {
    Challenge(String),
    Hello(u16),
    Heartbeat,
    Connection(String),
    Error(String),
}

async fn send_msg<W: AsyncWriteExt + Unpin>(w: &mut W, msg: &ClientMsg) -> std::io::Result<()> {
    let mut payload = serde_json::to_string(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    payload.push('\0');
    w.write_all(payload.as_bytes()).await?;
    w.flush().await
}

async fn recv_msg<R: AsyncBufReadExt + Unpin>(r: &mut R) -> std::io::Result<Option<ServerMsg>> {
    let mut buf = Vec::new();
    let n = r.read_until(0, &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&0) {
        buf.pop();
    }
    if buf.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// -- Auth --

struct Auth(HmacSha256);

impl Auth {
    fn new(secret: &str) -> Self {
        let key = Sha256::digest(secret.as_bytes());
        Self(HmacSha256::new_from_slice(&key).expect("hmac key"))
    }

    fn answer(&self, challenge: &str) -> String {
        let uuid = uuid::Uuid::parse_str(challenge).expect("invalid challenge UUID");
        let mut mac = self.0.clone();
        mac.update(uuid.as_bytes()); // raw 16 bytes, not string bytes
        hex::encode(mac.finalize().into_bytes())
    }
}

// -- Shared state --

struct Shared {
    state: String,
    remote_address: Option<String>,
    assigned_port: Option<u16>,
    logs: Vec<String>,
    last_error: Option<String>,
}

// -- Public types --

#[derive(Debug, Clone, serde::Serialize)]
pub struct TunnelStatus {
    pub state: String,
    pub local_address: String,
    pub remote_address: Option<String>,
    pub assigned_remote_port: Option<u16>,
    pub last_error: Option<String>,
    pub logs: Vec<String>,
}

impl Default for TunnelStatus {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            local_address: "127.0.0.1:25565".to_string(),
            remote_address: None,
            assigned_remote_port: None,
            last_error: None,
            logs: Vec::new(),
        }
    }
}

// -- Client --

pub struct BoreClient {
    shared: Arc<Mutex<Shared>>,
    active_conns: Arc<AtomicUsize>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl BoreClient {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                state: "idle".into(),
                remote_address: None,
                assigned_port: None,
                logs: Vec::new(),
                last_error: None,
            })),
            active_conns: Arc::new(AtomicUsize::new(0)),
            task: None,
        }
    }

    pub async fn status(&self) -> TunnelStatus {
        let s = self.shared.lock().await;
        TunnelStatus {
            state: s.state.clone(),
            local_address: "127.0.0.1:25565".into(),
            remote_address: s.remote_address.clone(),
            assigned_remote_port: s.assigned_port,
            last_error: s.last_error.clone(),
            logs: s.logs.clone(),
        }
    }

    pub async fn is_running(&self) -> bool {
        matches!(self.shared.lock().await.state.as_str(), "starting" | "connected")
    }

    pub async fn start(
        &mut self,
        server: &str,
        control_port: u16,
        local_port: u16,
        remote_port: u16,
        secret: &str,
    ) -> Result<(), String> {
        if self.is_running().await {
            return Err("Tunnel is already running.".into());
        }
        if server.is_empty() {
            return Err("Bore server host is required.".into());
        }

        {
            let mut s = self.shared.lock().await;
            s.state = "starting".into();
            s.remote_address = None;
            s.assigned_port = None;
            s.last_error = None;
            s.logs.clear();
            s.logs.push(format!("[bore] Connecting to {server}:{control_port}..."));
        }

        let shared = self.shared.clone();
        let conns = self.active_conns.clone();
        let srv = server.to_string();
        let sec = secret.to_string();

        self.task = Some(tokio::spawn(async move {
            if let Err(e) = control_loop(shared.clone(), conns, &srv, control_port, local_port, remote_port, &sec).await {
                let mut s = shared.lock().await;
                if s.state != "stopped" {
                    s.state = "failed".into();
                    s.last_error = Some(e.clone());
                    s.logs.push(format!("[error] {e}"));
                }
            }
        }));

        // Wait for connection to establish or fail
        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        let mut s = self.shared.lock().await;
        s.state = "stopped".into();
        s.last_error = None;
        s.logs.push("[app] Tunnel stopped.".into());
        Ok(())
    }
}

// -- TCP connect with timeout --

async fn tcp_connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    tokio::time::timeout(TIMEOUT, TcpStream::connect(format!("{host}:{port}")))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out"))?
}

// -- Auth handshake on a split stream --

async fn do_auth<R: AsyncBufReadExt + Unpin, W: AsyncWriteExt + Unpin>(
    r: &mut R,
    w: &mut W,
    secret: &str,
) -> Result<(String, String), String> {
    let challenge = match recv_msg(r).await.map_err(|e| format!("Auth read: {e}"))? {
        Some(ServerMsg::Challenge(uuid)) => uuid,
        Some(ServerMsg::Error(e)) => return Err(format!("Authentication failed. Check the server secret/password. ({e})")),
        other => return Err(format!("Expected Challenge, got {other:?}")),
    };
    let answer = Auth::new(secret).answer(&challenge);
    send_msg(w, &ClientMsg::Authenticate(answer.clone()))
        .await
        .map_err(|e| format!("Auth write: {e}"))?;
    w.flush().await.map_err(|e| format!("Auth flush: {e}"))?;
    Ok((challenge, answer))
}

// -- Control channel --

async fn control_loop(
    shared: Arc<Mutex<Shared>>,
    active_conns: Arc<AtomicUsize>,
    server: &str,
    control_port: u16,
    local_port: u16,
    remote_port: u16,
    secret: &str,
) -> Result<(), String> {
    let stream = tcp_connect(server, control_port)
        .await
        .map_err(|e| format!("Could not connect to tunnel server at {server}:{control_port}. {e}"))?;

    let (rh, mut wh) = tokio::io::split(stream);
    let mut reader = BufReader::new(rh);

    {
        let mut s = shared.lock().await;
        s.logs.push(format!("[bore] Connected to {server}:{control_port}"));
    }

    // Auth (skip for public servers with no secret)
    if !secret.is_empty() {
        let (challenge, answer) = do_auth(&mut reader, &mut wh, secret).await?;
        {
            let mut s = shared.lock().await;
            s.logs.push(format!("[bore] Challenge: {challenge}"));
            s.logs.push(format!("[bore] Answer: {answer}"));
            s.logs.push("[bore] Authenticated.".into());
        }
    } else {
        let mut s = shared.lock().await;
        s.logs.push("[bore] No secret provided, skipping authentication.".into());
    }

    // Hello
    send_msg(&mut wh, &ClientMsg::Hello(remote_port))
        .await
        .map_err(|e| format!("Hello send: {e}"))?;
    wh.flush().await.map_err(|e| format!("Hello flush: {e}"))?;

    let assigned = match recv_msg(&mut reader).await.map_err(|e| format!("Hello recv: {e}"))? {
        Some(ServerMsg::Hello(p)) => p,
        Some(ServerMsg::Error(e)) => return Err(format!("Remote server rejected: {e}. Try remote port 0 for automatic assignment.")),
        Some(ServerMsg::Challenge(_)) => return Err("Server requires authentication, but no secret was provided.".into()),
        None => return Err("Server closed connection during handshake.".into()),
        other => return Err(format!("Expected Hello, got {other:?}")),
    };

    let addr = format!("{server}:{assigned}");
    {
        let mut s = shared.lock().await;
        s.state = "connected".into();
        s.assigned_port = Some(assigned);
        s.remote_address = Some(addr.clone());
        s.logs.push(format!("[bore] Listening at {addr}"));
    }

    // Keep wh alive so TCP write side stays open (matches bore CLI behavior)

    // Main loop
    loop {
        match recv_msg(&mut reader).await {
            Ok(Some(ServerMsg::Heartbeat)) => {}
            Ok(Some(ServerMsg::Connection(id))) => {
                let sc = shared.clone();
                let cn = active_conns.clone();
                let srv = server.to_string();
                let sec = secret.to_string();
                tokio::spawn(async move {
                    cn.fetch_add(1, Ordering::Relaxed);
                    if let Err(e) = accept_connection(sc, &srv, control_port, &sec, local_port, &id).await {
                        eprintln!("[conn:{id}] {e}");
                    }
                    cn.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Ok(Some(ServerMsg::Error(e))) => {
                shared.lock().await.logs.push(format!("[bore] Server error: {e}"));
            }
            Ok(None) => {
                let mut s = shared.lock().await;
                s.state = "stopped".into();
                s.logs.push("[bore] Server closed connection.".into());
                return Ok(());
            }
            Ok(Some(other)) => {
                shared.lock().await.logs.push(format!("[bore] Unexpected: {other:?}"));
            }
            Err(e) => return Err(format!("Control read: {e}")),
        }
    }
}

// -- Accept one incoming connection and proxy to local Minecraft --

async fn accept_connection(
    shared: Arc<Mutex<Shared>>,
    server: &str,
    control_port: u16,
    secret: &str,
    local_port: u16,
    conn_id: &str,
) -> Result<(), String> {
    shared.lock().await.logs.push("[bore] Incoming connection".into());

    // Connect to local Minecraft FIRST (before accepting, so we don't consume server resources if local is down)
    let mut local = tcp_connect("127.0.0.1", local_port)
        .await
        .map_err(|e| format!("Local Minecraft server is not reachable at 127.0.0.1:{local_port}. {e}"))?;

    // New TCP connection to server
    let stream = tcp_connect(server, control_port)
        .await
        .map_err(|e| format!("Data connect to {server}:{control_port}: {e}"))?;

    let (rh, mut wh) = tokio::io::split(stream);
    let mut reader = BufReader::new(rh);

    // Auth + Accept (skip auth for public servers)
    if !secret.is_empty() {
        do_auth(&mut reader, &mut wh, secret).await?;
    }
    send_msg(&mut wh, &ClientMsg::Accept(conn_id.into()))
        .await
        .map_err(|e| format!("Accept send: {e}"))?;

    // Drain any bytes buffered in the BufReader
    let buffered: Vec<u8> = reader.buffer().to_vec();
    if !buffered.is_empty() {
        local.write_all(&buffered).await
            .map_err(|e| format!("Flush to local: {e}"))?;
    }

    // Get raw read half back from BufReader, keep write half
    let mut remote_read = reader.into_inner();
    let mut remote_write = wh;

    // Split local stream and do bidirectional copy via two tasks
    let (mut local_read, mut local_write) = tokio::io::split(local);
    let up = tokio::io::copy(&mut remote_read, &mut local_write);
    let down = tokio::io::copy(&mut local_read, &mut remote_write);
    let _ = tokio::try_join!(up, down);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn test_auth_answer_matches_bore() {
        // Verify HMAC computation matches the bore CLI implementation:
        // key = SHA256(secret), HMAC-SHA256(key, uuid_raw_16_bytes), hex encode
        let secret = "test-secret";
        let challenge_str = "550e8400-e29b-41d4-a716-446655440000";

        let auth = Auth::new(secret);
        let answer = auth.answer(challenge_str);

        // Manually compute expected result
        let key = Sha256::digest(secret.as_bytes());
        let uuid = uuid::Uuid::parse_str(challenge_str).unwrap();
        let mut mac = HmacSha256::new_from_slice(&key).unwrap();
        mac.update(uuid.as_bytes());
        let expected = hex::encode(mac.finalize().into_bytes());

        assert_eq!(answer, expected, "Auth answer should match manual HMAC computation");
    }

    #[test]
    fn test_auth_with_realistic_secret() {
        let secret = "nXhrn6hNyGV/fcQCyDWxpuNAERfs9P4a";
        let challenge_str = "12345678-1234-1234-1234-123456789abc";

        let auth = Auth::new(secret);
        let answer = auth.answer(challenge_str);

        // Verify the answer is a 64-char hex string (SHA256 HMAC)
        assert_eq!(answer.len(), 64);
        assert!(answer.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_auth_deterministic() {
        let secret = "my-secret";
        let challenge = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let auth = Auth::new(secret);
        let answer1 = auth.answer(challenge);
        let answer2 = auth.answer(challenge);

        assert_eq!(answer1, answer2, "Same input must produce same output");
    }

    #[test]
    fn test_uuid_bytes_not_string_bytes() {
        // Ensure we HMAC over raw 16 UUID bytes, not the string representation
        let secret = "secret";
        let challenge_str = "550e8400-e29b-41d4-a716-446655440000";

        let auth = Auth::new(secret);
        let answer_uuid_bytes = auth.answer(challenge_str);

        // Compute with string bytes (wrong way)
        let key = Sha256::digest(secret.as_bytes());
        let mut mac = HmacSha256::new_from_slice(&key).unwrap();
        mac.update(challenge_str.as_bytes()); // string bytes, NOT uuid raw bytes
        let wrong_answer = hex::encode(mac.finalize().into_bytes());

        assert_ne!(answer_uuid_bytes, wrong_answer, "Must use UUID raw bytes, not string");
    }

    #[test]
    fn test_auth_self_validate() {
        // Mirror bore's own test: answer should validate against same challenge
        let secret = "test";
        let challenge_str = "00000000-0000-0000-0000-000000000001";

        let auth = Auth::new(secret);
        let answer = auth.answer(challenge_str);

        // Validate by recomputing
        let key = Sha256::digest(secret.as_bytes());
        let uuid = uuid::Uuid::parse_str(challenge_str).unwrap();
        let mut mac = HmacSha256::new_from_slice(&key).unwrap();
        mac.update(uuid.as_bytes());
        let tag_bytes = mac.finalize().into_bytes();

        let answer_bytes = hex::decode(&answer).unwrap();
        assert_eq!(tag_bytes.as_slice(), answer_bytes.as_slice());
    }

    #[test]
    fn test_wire_format_authenticate() {
        let answer = "abc123";
        let msg = ClientMsg::Authenticate(answer.to_string());
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"Authenticate":"abc123"}"#);
    }

    #[test]
    fn test_wire_format_hello() {
        let msg = ClientMsg::Hello(0u16);
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"Hello":0}"#);
    }

    #[test]
    fn test_wire_format_accept() {
        let msg = ClientMsg::Accept("550e8400-e29b-41d4-a716-446655440000".to_string());
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"Accept":"550e8400-e29b-41d4-a716-446655440000"}"#);
    }

    #[test]
    fn test_wire_format_parse_challenge() {
        let json = r#"{"Challenge":"550e8400-e29b-41d4-a716-446655440000"}"#;
        let msg: ServerMsg = serde_json::from_str(json).unwrap();
        match msg {
            ServerMsg::Challenge(uuid) => {
                assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
            }
            _ => panic!("Expected Challenge, got {:?}", msg),
        }
    }

    #[test]
    fn test_wire_format_parse_hello() {
        let json = r#"{"Hello":12345}"#;
        let msg: ServerMsg = serde_json::from_str(json).unwrap();
        match msg {
            ServerMsg::Hello(p) => assert_eq!(p, 12345),
            _ => panic!("Expected Hello, got {:?}", msg),
        }
    }

    #[test]
    fn test_wire_format_parse_error() {
        let json = r#"{"Error":"invalid secret"}"#;
        let msg: ServerMsg = serde_json::from_str(json).unwrap();
        match msg {
            ServerMsg::Error(e) => assert_eq!(e, "invalid secret"),
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }
}

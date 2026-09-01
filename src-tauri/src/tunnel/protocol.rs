//! Wire protocol for the Spore Tunnel control channel.
//!
//! Framing is length-delimited JSON: a `u32` little-endian byte count
//! followed by that many bytes of compact JSON. Message envelopes are
//! externally tagged and byte-compatible with `bore` servers, plus a
//! `spore` dialect that sends some values untagged.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::tunnel::TunnelError;

/// Largest frame payload we accept on the wire (1 MiB).
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Size of the chunks [`FrameReader`] pulls from the stream.
const READ_CHUNK: usize = 8 * 1024;

/// Errors raised while encoding, decoding or interpreting wire traffic.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// A frame prefix announced a payload larger than [`MAX_FRAME_LEN`].
    #[error("frame length {0} exceeds maximum {MAX_FRAME_LEN}")]
    FrameTooLarge(usize),

    /// A payload was not acceptable JSON for the expected message type.
    #[error("malformed json: {0}")]
    MalformedJson(String),

    /// The underlying stream failed.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

impl From<ProtocolError> for TunnelError {
    fn from(e: ProtocolError) -> Self {
        TunnelError::Protocol(e.to_string())
    }
}

/// Serializes `msg` and prefixes it with its byte length as a `u32`
/// little-endian header.
pub fn encode_frame<T: serde::Serialize + ?Sized>(msg: &T) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_vec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "payload exceeds u32 length"))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Incremental decoder for length-prefixed frames fed from arbitrary,
/// possibly fragmented byte chunks.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    /// Bytes received so far, headed by the frame currently being assembled.
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Creates an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends raw bytes received from the peer.
    ///
    /// Bytes belonging to a partial frame stay buffered until later pushes
    /// complete them.
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Returns the next complete frame payload, if one is fully buffered.
    ///
    /// Returns `Ok(None)` while the buffered data holds only a partial
    /// header or payload. An oversized length prefix is rejected from the
    /// prefix alone, before any payload arrives.
    pub fn next_payload(&mut self) -> Result<Option<Vec<u8>>, ProtocolError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let header: [u8; 4] = self.buf[..4].try_into().expect("four header bytes");
        let len = u32::from_le_bytes(header) as usize;
        if len > MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(len));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(payload))
    }
}

/// Reads length-prefixed frames from an async byte stream.
pub struct FrameReader<R> {
    stream: R,
    decoder: FrameDecoder,
    chunk: Box<[u8; READ_CHUNK]>,
}

impl<R: tokio::io::AsyncRead + Unpin> FrameReader<R> {
    /// Wraps `stream` in a frame reader.
    pub fn new(stream: R) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            chunk: Box::new([0u8; READ_CHUNK]),
        }
    }

    /// Reads the next complete frame payload.
    ///
    /// Returns `Ok(None)` on a clean end of stream. A frame announcing more
    /// than [`MAX_FRAME_LEN`] bytes surfaces as
    /// [`io::ErrorKind::InvalidData`].
    pub async fn next_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            match self.decoder.next_payload() {
                Ok(Some(payload)) => return Ok(Some(payload)),
                Ok(None) => {}
                Err(e @ ProtocolError::FrameTooLarge(_)) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, e.to_string()));
                }
                Err(ProtocolError::Io(e)) => return Err(e),
                Err(e) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, e.to_string()));
                }
            }

            let n = self.stream.read(self.chunk.as_mut()).await?;
            if n == 0 {
                return Ok(None);
            }
            self.decoder.push(&self.chunk[..n]);
        }
    }
}

/// Writes `payload` as a single length-prefixed frame and flushes.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "payload exceeds u32 length"))?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(payload).await?;
    w.flush().await
}

/// Client version advertised in `HelloEx`.
pub const CLIENT_VERSION: &str = "spore/1";

/// Bare-string payload both dialects use to acknowledge a message.
pub const ACK_PAYLOAD: &str = "Ack";

/// Messages the client sends to the server.
///
/// Serde's derived externally-tagged forms are already wire-compatible with
/// bore: `{"Hello":12345}`, `{"HelloEx":{...}}`, `{"Heartbeat":null}`, ...
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClientMessage {
    /// HMAC answer to the server's challenge.
    Authenticate(String),
    /// bore dialect port announcement.
    Hello(u16),
    /// spore dialect handshake with version and feature negotiation.
    HelloEx {
        /// Local port to expose.
        port: u16,
        /// Client version string, e.g. [`CLIENT_VERSION`].
        version: String,
        /// Feature names the client supports.
        features: Vec<String>,
    },
    /// Keepalive ping.
    Heartbeat,
    /// Accept an offered forwarded connection by id.
    Accept(String),
}

/// Messages the server sends to the client.
///
/// Serialization and deserialization are hand-written so the type tolerates
/// both bore (always tagged) and spore (sometimes bare) dialects: challenges
/// may arrive as a bare string, `Ack` may arrive as either form, and unknown
/// fields or variants of the other dialect are ignored.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// Auth challenge nonce.
    Challenge(String),
    /// bore dialect port assignment.
    Hello(u16),
    /// spore dialect handshake reply with negotiated features.
    HelloEx {
        /// Assigned or confirmed port.
        port: u16,
        /// Server feature names; absent in older dialects.
        features: Vec<String>,
    },
    /// Acknowledgement (serialized as the bare string `"Ack"`).
    Ack,
    /// Keepalive reply.
    Heartbeat,
    /// Id of a forwarded connection the client should accept.
    Connection(String),
    /// Server-side error report.
    Error(String),
}

/// Wire body of a server `HelloEx`.
#[derive(serde::Serialize)]
struct HelloExBody<'a> {
    port: u16,
    features: &'a [String],
}

/// Lenient wire body of a server `HelloEx`: unknown fields ignored,
/// `features` defaults to empty.
#[derive(serde::Deserialize)]
struct HelloExWire {
    port: u16,
    #[serde(default)]
    features: Vec<String>,
}

/// Serializes a one-entry externally-tagged object: `{"<tag>":value}`.
fn tagged<S, V>(serializer: S, tag: &str, value: &V) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    V: serde::Serialize + ?Sized,
{
    use serde::ser::SerializeMap;

    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(tag, value)?;
    map.end()
}

impl serde::Serialize for ServerMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ServerMessage::Challenge(nonce) => tagged(serializer, "Challenge", nonce),
            ServerMessage::Hello(port) => tagged(serializer, "Hello", port),
            ServerMessage::HelloEx { port, features } => {
                let body = HelloExBody {
                    port: *port,
                    features,
                };
                tagged(serializer, "HelloEx", &body)
            }
            ServerMessage::Ack => serializer.serialize_str(ACK_PAYLOAD),
            ServerMessage::Heartbeat => tagged(serializer, "Heartbeat", &()),
            ServerMessage::Connection(id) => tagged(serializer, "Connection", id),
            ServerMessage::Error(msg) => tagged(serializer, "Error", msg),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ServerMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ServerMessageVisitor;

        impl<'de> serde::de::Visitor<'de> for ServerMessageVisitor {
            type Value = ServerMessage;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bore/spore server message (tagged object or bare string)")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value == ACK_PAYLOAD {
                    Ok(ServerMessage::Ack)
                } else {
                    // Spore servers send the challenge nonce untagged.
                    Ok(ServerMessage::Challenge(value.to_string()))
                }
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut found: Option<ServerMessage> = None;
                while let Some(key) = map.next_key::<String>()? {
                    let msg = match key.as_str() {
                        "Hello" => ServerMessage::Hello(map.next_value()?),
                        "HelloEx" => {
                            let body = map.next_value::<HelloExWire>()?;
                            ServerMessage::HelloEx {
                                port: body.port,
                                features: body.features,
                            }
                        }
                        "Challenge" => ServerMessage::Challenge(map.next_value()?),
                        "Connection" => ServerMessage::Connection(map.next_value()?),
                        "Error" => ServerMessage::Error(map.next_value()?),
                        "Ack" => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            ServerMessage::Ack
                        }
                        "Heartbeat" => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            ServerMessage::Heartbeat
                        }
                        // Unknown top-level keys (the other dialect's
                        // extras) are ignored.
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            continue;
                        }
                    };
                    found.get_or_insert(msg);
                }
                found.ok_or_else(|| serde::de::Error::custom("unrecognized server message"))
            }
        }

        deserializer.deserialize_any(ServerMessageVisitor)
    }
}

/// Parses a client message from a frame payload.
pub fn parse_client_message(payload: &[u8]) -> Result<ClientMessage, ProtocolError> {
    serde_json::from_slice(payload).map_err(|e| ProtocolError::MalformedJson(e.to_string()))
}

/// Parses a server message from a frame payload.
pub fn parse_server_message(payload: &[u8]) -> Result<ServerMessage, ProtocolError> {
    serde_json::from_slice(payload).map_err(|e| ProtocolError::MalformedJson(e.to_string()))
}

/// Computes the bore-compatible answer to a server challenge.
///
/// The HMAC key is `SHA256(secret)` and the message is the challenge
/// nonce's 16 raw UUID bytes. If the nonce is not a parseable UUID, the
/// nonce's string bytes are used instead. The result is lowercase hex.
pub fn challenge_answer(secret: &str, nonce: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    let key = Sha256::digest(secret.as_bytes());
    let mut mac = <Hmac<Sha256>>::new_from_slice(&key).expect("HMAC-SHA256 accepts any key size");
    match uuid::Uuid::parse_str(nonce) {
        Ok(id) => mac.update(id.as_bytes()),
        Err(_) => mac.update(nonce.as_bytes()),
    }
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// Minimal serializable probe used by the framing tests.
    #[derive(serde::Serialize)]
    struct Probe {
        hello: u16,
    }

    /// In-memory [`tokio::io::AsyncWrite`] sink: records every flushed byte.
    struct VecSink(Vec<u8>);

    impl tokio::io::AsyncWrite for VecSink {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.get_mut().0.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn encode_frame_writes_le_u32_length_prefix_then_payload() {
        let frame = encode_frame(&Probe { hello: 12345 }).unwrap();
        let payload = br#"{"hello":12345}"#;
        let mut expected = Vec::with_capacity(4 + payload.len());
        expected.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        expected.extend_from_slice(payload);
        assert_eq!(frame, expected);
    }

    #[test]
    fn decoder_default_and_new_start_empty() {
        let mut dec = FrameDecoder::default();
        assert!(dec.next_payload().unwrap().is_none());
        let mut dec = FrameDecoder::new();
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_roundtrips_single_frame() {
        let frame = encode_frame(&Probe { hello: 1 }).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&frame);
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":1}"#);
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_byte_by_byte_feed_stays_none_until_complete() {
        let frame = encode_frame(&Probe { hello: 7 }).unwrap();
        let mut dec = FrameDecoder::new();
        for (i, byte) in frame.iter().enumerate() {
            dec.push(&[*byte]);
            if i + 1 < frame.len() {
                assert!(
                    dec.next_payload().unwrap().is_none(),
                    "frame considered complete early, after byte {i}"
                );
            }
        }
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":7}"#);
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_returns_two_frames_pushed_together() {
        let mut wire = encode_frame(&Probe { hello: 1 }).unwrap();
        wire.extend(encode_frame(&Probe { hello: 2 }).unwrap());
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":1}"#);
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":2}"#);
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_preserves_partial_frame_across_pushes() {
        let frame = encode_frame(&Probe { hello: 9 }).unwrap();
        let (head, tail) = frame.split_at(frame.len() / 2);
        let mut dec = FrameDecoder::new();
        dec.push(head);
        assert!(dec.next_payload().unwrap().is_none());
        dec.push(tail);
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":9}"#);
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_keeps_partial_second_frame_after_consuming_first() {
        let first = encode_frame(&Probe { hello: 1 }).unwrap();
        let second = encode_frame(&Probe { hello: 2 }).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&first);
        dec.push(&second[..second.len() - 1]);
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":1}"#);
        // The truncated second frame stays buffered and incomplete.
        assert!(dec.next_payload().unwrap().is_none());
        dec.push(&second[second.len() - 1..]);
        assert_eq!(dec.next_payload().unwrap().unwrap(), br#"{"hello":2}"#);
    }

    #[test]
    fn decoder_rejects_oversize_prefix_without_waiting_for_payload() {
        let mut dec = FrameDecoder::new();
        dec.push(&((MAX_FRAME_LEN + 1) as u32).to_le_bytes());
        match dec.next_payload() {
            Err(ProtocolError::FrameTooLarge(n)) => assert_eq!(n, MAX_FRAME_LEN + 1),
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn decoder_accepts_prefix_at_exactly_max_frame_len() {
        let mut dec = FrameDecoder::new();
        dec.push(&(MAX_FRAME_LEN as u32).to_le_bytes());
        // At the limit the frame is legal: the decoder just keeps waiting
        // for the payload instead of erroring.
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_handles_header_split_across_pushes() {
        for split in 1..=3usize {
            let frame = encode_frame(&Probe { hello: 3 }).unwrap();
            let mut dec = FrameDecoder::new();
            dec.push(&frame[..split]);
            assert!(dec.next_payload().unwrap().is_none());
            dec.push(&frame[split..]);
            assert_eq!(
                dec.next_payload().unwrap().unwrap(),
                br#"{"hello":3}"#,
                "failed with header split after {split} bytes"
            );
        }
    }

    #[tokio::test]
    async fn frame_reader_yields_frames_then_clean_eof() {
        let mut wire = encode_frame(&Probe { hello: 1 }).unwrap();
        wire.extend(encode_frame(&Probe { hello: 2 }).unwrap());
        let mut reader = FrameReader::new(wire.as_slice());
        assert_eq!(
            reader.next_frame().await.unwrap().unwrap(),
            br#"{"hello":1}"#.to_vec()
        );
        assert_eq!(
            reader.next_frame().await.unwrap().unwrap(),
            br#"{"hello":2}"#.to_vec()
        );
        assert!(reader.next_frame().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn frame_reader_maps_oversize_frame_to_invalid_data() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&((MAX_FRAME_LEN + 1) as u32).to_le_bytes());
        wire.extend_from_slice(b"way too much data");
        let mut reader = FrameReader::new(wire.as_slice());
        let err = reader.next_frame().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn frame_reader_reports_truncated_tail_as_eof() {
        // A clean EOF while a frame is still incomplete yields `None`,
        // not an error.
        let frame = encode_frame(&Probe { hello: 4 }).unwrap();
        let mut reader = FrameReader::new(frame[..frame.len() - 1].as_ref());
        assert!(reader.next_frame().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn write_frame_emits_prefix_then_payload() {
        let mut sink = VecSink(Vec::new());
        write_frame(&mut sink, br#"{"hello":1}"#).await.unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&(11u32).to_le_bytes());
        expected.extend_from_slice(br#"{"hello":1}"#);
        assert_eq!(sink.0, expected);
    }

    #[tokio::test]
    async fn write_frame_roundtrips_through_frame_reader() {
        let mut sink = VecSink(Vec::new());
        write_frame(&mut sink, b"ping").await.unwrap();
        write_frame(&mut sink, b"pong").await.unwrap();
        let mut reader = FrameReader::new(sink.0.as_slice());
        assert_eq!(reader.next_frame().await.unwrap().unwrap(), b"ping".to_vec());
        assert_eq!(reader.next_frame().await.unwrap().unwrap(), b"pong".to_vec());
        assert!(reader.next_frame().await.unwrap().is_none());
    }

    // --- messages --------------------------------------------------------

    fn client_variants() -> Vec<ClientMessage> {
        vec![
            ClientMessage::Authenticate("deadbeef".to_string()),
            ClientMessage::Hello(12345),
            ClientMessage::HelloEx {
                port: 1,
                version: CLIENT_VERSION.to_string(),
                features: vec![],
            },
            ClientMessage::HelloEx {
                port: 2,
                version: "spore/2".to_string(),
                features: vec!["keepalive".to_string()],
            },
            ClientMessage::Heartbeat,
            ClientMessage::Accept("550e8400-e29b-41d4-a716-446655440000".to_string()),
        ]
    }

    fn server_variants() -> Vec<ServerMessage> {
        vec![
            ServerMessage::Challenge("nonce-1".to_string()),
            ServerMessage::Hello(25565),
            ServerMessage::HelloEx {
                port: 1,
                features: vec![],
            },
            ServerMessage::HelloEx {
                port: 2,
                features: vec!["ex".to_string()],
            },
            ServerMessage::Ack,
            ServerMessage::Heartbeat,
            ServerMessage::Connection("conn-7".to_string()),
            ServerMessage::Error("boom".to_string()),
        ]
    }

    #[test]
    fn client_message_wire_snapshots() {
        let cases = [
            (
                ClientMessage::Authenticate("deadbeef".to_string()),
                r#"{"Authenticate":"deadbeef"}"#,
            ),
            (ClientMessage::Hello(12345), r#"{"Hello":12345}"#),
            (
                ClientMessage::HelloEx {
                    port: 1,
                    version: CLIENT_VERSION.to_string(),
                    features: vec![],
                },
                r#"{"HelloEx":{"port":1,"version":"spore/1","features":[]}}"#,
            ),
            (
                ClientMessage::HelloEx {
                    port: 2,
                    version: "v2".to_string(),
                    features: vec!["a".to_string(), "b".to_string()],
                },
                r#"{"HelloEx":{"port":2,"version":"v2","features":["a","b"]}}"#,
            ),
            // serde's derived externally-tagged form for a unit variant is
            // the bare string — this is exactly what bore clients emit.
            (ClientMessage::Heartbeat, r#""Heartbeat""#),
            (
                ClientMessage::Accept("550e8400-e29b-41d4-a716-446655440000".to_string()),
                r#"{"Accept":"550e8400-e29b-41d4-a716-446655440000"}"#,
            ),
        ];
        for (msg, wire) in cases {
            assert_eq!(
                serde_json::to_string(&msg).unwrap(),
                wire,
                "snapshot mismatch for {msg:?}"
            );
        }
    }

    #[test]
    fn client_parse_accepts_both_heartbeat_forms() {
        // The canonical derived form is the bare string; peers following
        // the {"Heartbeat":null} spelling must still be understood.
        assert_eq!(
            parse_client_message(br#""Heartbeat""#).unwrap(),
            ClientMessage::Heartbeat
        );
        assert_eq!(
            parse_client_message(br#"{"Heartbeat":null}"#).unwrap(),
            ClientMessage::Heartbeat
        );
    }

    #[test]
    fn client_version_is_spore_one() {
        assert_eq!(CLIENT_VERSION, "spore/1");
    }

    #[test]
    fn server_message_canonical_snapshots() {
        let cases = [
            (
                ServerMessage::Challenge("nonce".to_string()),
                r#"{"Challenge":"nonce"}"#,
            ),
            (ServerMessage::Hello(25565), r#"{"Hello":25565}"#),
            (
                ServerMessage::HelloEx {
                    port: 1,
                    features: vec![],
                },
                r#"{"HelloEx":{"port":1,"features":[]}}"#,
            ),
            (
                ServerMessage::HelloEx {
                    port: 2,
                    features: vec!["ex".to_string()],
                },
                r#"{"HelloEx":{"port":2,"features":["ex"]}}"#,
            ),
            (ServerMessage::Ack, r#""Ack""#),
            (ServerMessage::Heartbeat, r#"{"Heartbeat":null}"#),
            (
                ServerMessage::Connection("c-1".to_string()),
                r#"{"Connection":"c-1"}"#,
            ),
            (ServerMessage::Error("boom".to_string()), r#"{"Error":"boom"}"#),
        ];
        for (msg, wire) in cases {
            assert_eq!(
                serde_json::to_string(&msg).unwrap(),
                wire,
                "canonical form mismatch for {msg:?}"
            );
        }
    }

    #[test]
    fn server_ack_serializes_as_bare_ack_payload() {
        assert_eq!(ACK_PAYLOAD, "Ack");
        assert_eq!(
            serde_json::to_string(&ServerMessage::Ack).unwrap(),
            format!("\"{ACK_PAYLOAD}\"")
        );
    }

    #[test]
    fn server_parses_tagged_challenge() {
        assert_eq!(
            parse_server_message(br#"{"Challenge":"abc"}"#).unwrap(),
            ServerMessage::Challenge("abc".to_string())
        );
    }

    #[test]
    fn server_parses_bare_string_as_challenge() {
        assert_eq!(
            parse_server_message(br#""abc""#).unwrap(),
            ServerMessage::Challenge("abc".to_string())
        );
    }

    #[test]
    fn server_parses_ack_bare_and_tagged() {
        assert_eq!(parse_server_message(br#""Ack""#).unwrap(), ServerMessage::Ack);
        assert_eq!(parse_server_message(br#"{"Ack":null}"#).unwrap(), ServerMessage::Ack);
        assert_eq!(
            parse_server_message(br#"{"Ack":"anything"}"#).unwrap(),
            ServerMessage::Ack
        );
    }

    #[test]
    fn server_parses_heartbeat_with_any_payload() {
        assert_eq!(
            parse_server_message(br#"{"Heartbeat":null}"#).unwrap(),
            ServerMessage::Heartbeat
        );
        assert_eq!(
            parse_server_message(br#"{"Heartbeat":1}"#).unwrap(),
            ServerMessage::Heartbeat
        );
    }

    #[test]
    fn server_parses_hello() {
        assert_eq!(
            parse_server_message(br#"{"Hello":80}"#).unwrap(),
            ServerMessage::Hello(80)
        );
    }

    #[test]
    fn server_parses_hello_ex_leniently() {
        assert_eq!(
            parse_server_message(br#"{"HelloEx":{"port":7,"features":["f"],"unknown":true}}"#)
                .unwrap(),
            ServerMessage::HelloEx {
                port: 7,
                features: vec!["f".to_string()]
            }
        );
        assert_eq!(
            parse_server_message(br#"{"HelloEx":{"port":7}}"#).unwrap(),
            ServerMessage::HelloEx {
                port: 7,
                features: vec![]
            }
        );
    }

    #[test]
    fn server_parses_connection_and_error() {
        assert_eq!(
            parse_server_message(br#"{"Connection":"c1"}"#).unwrap(),
            ServerMessage::Connection("c1".to_string())
        );
        assert_eq!(
            parse_server_message(br#"{"Error":"nope"}"#).unwrap(),
            ServerMessage::Error("nope".to_string())
        );
    }

    #[test]
    fn server_ignores_unknown_top_level_keys() {
        assert_eq!(
            parse_server_message(br#"{"Hello":9,"Extra":"x"}"#).unwrap(),
            ServerMessage::Hello(9)
        );
        assert_eq!(
            parse_server_message(br#"{"Extra":"x","Challenge":"n"}"#).unwrap(),
            ServerMessage::Challenge("n".to_string())
        );
    }

    #[test]
    fn server_rejects_unknown_variants() {
        assert!(matches!(
            parse_server_message(br#"{"Unknown":1}"#),
            Err(ProtocolError::MalformedJson(_))
        ));
        assert!(matches!(
            parse_server_message(b"{}"),
            Err(ProtocolError::MalformedJson(_))
        ));
    }

    #[test]
    fn server_rejects_bad_shapes() {
        let wires = [
            &br#"{"Hello":"not-a-number"}"#[..],
            br#"{"Challenge":5}"#,
            br#"{"HelloEx":"not-an-object"}"#,
            br#"{"HelloEx":{"port":"1"}}"#,
            br#"{"Connection":42}"#,
        ];
        for wire in wires {
            assert!(
                matches!(parse_server_message(wire), Err(ProtocolError::MalformedJson(_))),
                "expected rejection of {wire:?}"
            );
        }
    }

    #[test]
    fn server_rejects_non_object_non_string_json() {
        let wires = [
            &b"5"[..],
            b"[1,2]",
            b"true",
            b"null",
            b"not json",
        ];
        for wire in wires {
            assert!(
                matches!(parse_server_message(wire), Err(ProtocolError::MalformedJson(_))),
                "expected rejection of {wire:?}"
            );
        }
    }

    #[test]
    fn client_parse_rejects_bad_json_and_shapes() {
        assert!(matches!(
            parse_client_message(b"{"),
            Err(ProtocolError::MalformedJson(_))
        ));
        assert!(matches!(
            parse_client_message(br#"{"Unknown":1}"#),
            Err(ProtocolError::MalformedJson(_))
        ));
        assert!(matches!(
            parse_client_message(br#"{"Hello":"x"}"#),
            Err(ProtocolError::MalformedJson(_))
        ));
    }

    #[test]
    fn client_roundtrips_through_frame_codec() {
        for msg in client_variants() {
            let bytes = encode_frame(&msg).unwrap();
            let mut dec = FrameDecoder::new();
            dec.push(&bytes);
            let payload = dec.next_payload().unwrap().unwrap();
            assert_eq!(parse_client_message(&payload).unwrap(), msg);
        }
    }

    #[test]
    fn server_roundtrips_through_serialize_and_parse() {
        for msg in server_variants() {
            let wire = serde_json::to_string(&msg).unwrap();
            assert_eq!(
                parse_server_message(wire.as_bytes()).unwrap(),
                msg,
                "roundtrip failed for {wire}"
            );
        }
    }

    #[test]
    fn protocol_error_converts_to_tunnel_error() {
        let err = ProtocolError::MalformedJson("x".to_string());
        let tunnel = TunnelError::from(err);
        assert!(matches!(tunnel, TunnelError::Protocol(_)));
        assert_eq!(tunnel.to_string(), "protocol error: malformed json: x");

        let err = ProtocolError::FrameTooLarge(MAX_FRAME_LEN + 1);
        assert!(matches!(TunnelError::from(err), TunnelError::Protocol(_)));
    }

    // --- challenge auth --------------------------------------------------

    const AUTH_SECRET: &str = "hunter2";
    const AUTH_NONCE: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn challenge_answer_matches_independent_hmac_computation() {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};

        let key = Sha256::digest(AUTH_SECRET.as_bytes());
        let mut mac = <Hmac<Sha256>>::new_from_slice(&key).unwrap();
        mac.update(uuid::Uuid::parse_str(AUTH_NONCE).unwrap().as_bytes());
        let expected = hex::encode(mac.finalize().into_bytes());

        let answer = challenge_answer(AUTH_SECRET, AUTH_NONCE);
        assert_eq!(answer, expected);
        assert_eq!(answer.len(), 64);
        assert!(answer.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn challenge_answer_uses_uuid_raw_bytes_not_nonce_string() {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};

        let key = Sha256::digest(AUTH_SECRET.as_bytes());
        let mut mac = <Hmac<Sha256>>::new_from_slice(&key).unwrap();
        mac.update(AUTH_NONCE.as_bytes());
        let over_string = hex::encode(mac.finalize().into_bytes());

        assert_ne!(challenge_answer(AUTH_SECRET, AUTH_NONCE), over_string);
    }

    #[test]
    fn challenge_answer_is_stable() {
        assert_eq!(
            challenge_answer(AUTH_SECRET, AUTH_NONCE),
            challenge_answer(AUTH_SECRET, AUTH_NONCE)
        );
        assert_ne!(
            challenge_answer(AUTH_SECRET, AUTH_NONCE),
            challenge_answer("other-secret", AUTH_NONCE)
        );
        assert_ne!(
            challenge_answer(AUTH_SECRET, AUTH_NONCE),
            challenge_answer(AUTH_SECRET, "550e8400-e29b-41d4-a716-446655449999")
        );
        // Hyphenated and compact spellings of the same UUID are the same
        // 16 raw bytes, so they must produce the same answer.
        assert_eq!(
            challenge_answer(AUTH_SECRET, AUTH_NONCE),
            challenge_answer(AUTH_SECRET, "550e8400e29b41d4a716446655440000")
        );
    }

    #[test]
    fn challenge_answer_falls_back_to_string_bytes_for_non_uuid_nonce() {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};

        let nonce = "not-a-uuid";
        assert!(uuid::Uuid::parse_str(nonce).is_err());

        let key = Sha256::digest(AUTH_SECRET.as_bytes());
        let mut mac = <Hmac<Sha256>>::new_from_slice(&key).unwrap();
        mac.update(nonce.as_bytes());
        let expected = hex::encode(mac.finalize().into_bytes());

        assert_eq!(challenge_answer(AUTH_SECRET, nonce), expected);
    }
}

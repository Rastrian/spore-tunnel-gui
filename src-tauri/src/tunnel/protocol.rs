//! Wire protocol for the Spore Tunnel control channel.
//!
//! Framing is null-delimited JSON: each message is a compact JSON document
//! terminated by a single NUL byte (`0x00`), with frames capped at
//! [`MAX_FRAME_LEN`] — byte-compatible with `bore` servers
//! (`AnyDelimiterCodec` over delimiters `[0]`, max length 256) and Spore's
//! `Delimited` transport (`lib/spore/shared.ex`). Message envelopes are
//! externally tagged; the spore dialect sends some values untagged.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::tunnel::TunnelError;

/// Largest frame we accept on the wire, matching bore's `MAX_FRAME_LENGTH`
/// and Spore's `max_frame_length`.
pub const MAX_FRAME_LEN: usize = 256;

/// Size of the chunks [`FrameReader`] pulls from the stream.
const READ_CHUNK: usize = 8 * 1024;

/// Errors raised while encoding, decoding or interpreting wire traffic.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// More than [`MAX_FRAME_LEN`] bytes buffered without a delimiter.
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

/// Serializes `msg` and appends the NUL delimiter.
pub fn encode_frame<T: serde::Serialize + ?Sized>(msg: &T) -> io::Result<Vec<u8>> {
    let mut frame = serde_json::to_vec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    ensure_sendable(frame.len())?;
    frame.push(0);
    Ok(frame)
}

/// Rejects payloads whose frame (payload + delimiter) could not be decoded
/// by the strictest receiver: Spore's `read_frame` fails on buffers over
/// [`MAX_FRAME_LEN`] bytes *including* the delimiter.
fn ensure_sendable(payload_len: usize) -> io::Result<()> {
    if payload_len + 1 > MAX_FRAME_LEN {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {payload_len} payload bytes exceeds maximum {MAX_FRAME_LEN}"),
        ))
    } else {
        Ok(())
    }
}

/// Incremental decoder for NUL-delimited frames fed from arbitrary,
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

    /// Returns the payload of the next complete frame (the bytes before the
    /// next NUL delimiter), if one is fully buffered.
    ///
    /// Returns `Ok(None)` while no delimiter has arrived yet. More than
    /// [`MAX_FRAME_LEN`] bytes without a delimiter is rejected before the
    /// frame completes, mirroring bore's `AnyDelimiterCodec`.
    pub fn next_payload(&mut self) -> Result<Option<Vec<u8>>, ProtocolError> {
        if let Some(end) = self.buf.iter().position(|&b| b == 0) {
            let payload = self.buf[..end].to_vec();
            self.buf.drain(..=end);
            return Ok(Some(payload));
        }
        if self.buf.len() > MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(self.buf.len()));
        }
        Ok(None)
    }

    /// Returns and clears every buffered byte, complete frames and partials
    /// alike.
    ///
    /// Used when a connection switches from framed control traffic to a raw
    /// byte stream (after `Accept`): visitor bytes that arrived in the same
    /// TCP segment as the last frame must not be lost.
    pub fn drain_buffer(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

/// Reads length-prefixed frames from an async byte stream.
#[derive(Debug)]
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
    /// Returns `Ok(None)` on a clean end of stream. A frame exceeding
    /// [`MAX_FRAME_LEN`] bytes without a delimiter surfaces as
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

    /// Next inbound server message; `Ok(None)` on a clean end of stream.
    ///
    /// Malformed JSON surfaces as [`TunnelError::Protocol`], stream failures
    /// as [`TunnelError::Io`].
    pub async fn next_server_message(&mut self) -> Result<Option<ServerMessage>, TunnelError> {
        self.next_typed(parse_server_message).await
    }

    /// Next inbound client message; `Ok(None)` on a clean end of stream.
    pub async fn next_client_message(&mut self) -> Result<Option<ClientMessage>, TunnelError> {
        self.next_typed(parse_client_message).await
    }

    async fn next_typed<T>(
        &mut self,
        parse: fn(&[u8]) -> Result<T, ProtocolError>,
    ) -> Result<Option<T>, TunnelError> {
        match self.next_frame().await? {
            None => Ok(None),
            Some(payload) => parse(&payload).map(Some).map_err(TunnelError::from),
        }
    }

    /// Decomposes into the underlying stream and the decoder (with any
    /// buffered bytes intact) — for handing a connection back to raw mode.
    pub fn into_parts(self) -> (R, FrameDecoder) {
        (self.stream, self.decoder)
    }
}

/// Writes `payload` plus a single NUL delimiter and flushes.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    ensure_sendable(payload.len())?;
    w.write_all(payload).await?;
    w.write_all(&[0]).await?;
    w.flush().await
}

/// Serializes `msg` and writes it as one framed message.
///
/// Note: `encode_frame` already prefixes; passing its output to
/// [`write_frame`] would prefix twice. This helper does it right.
pub async fn send<W: AsyncWriteExt + Unpin, T: serde::Serialize + ?Sized>(
    w: &mut W,
    msg: &T,
) -> io::Result<()> {
    let payload =
        serde_json::to_vec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_frame(w, &payload).await
}

/// Client version advertised in `HelloEx`.
pub const CLIENT_VERSION: &str = "spore/1";

/// Bare-string payload both dialects use to acknowledge a message.
pub const ACK_PAYLOAD: &str = "Ack";

/// Bare-string keepalive both real server dialects send periodically.
pub const HEARTBEAT_PAYLOAD: &str = "Heartbeat";

/// Messages the client sends to the server.
///
/// Serde's derived externally-tagged forms are already wire-compatible with
/// bore: `{"Hello":12345}`, `{"Authenticate":"..."}`, `{"Accept":"..."}`,
/// plus the spore-only `{"HelloEx":{...}}`.
///
/// There is deliberately **no client `Heartbeat` variant**: real bore
/// servers decode `ClientMessage` as a closed enum and drop the connection
/// on unknown variants, and Spore expects client silence on the control
/// plane after the handshake. Liveness is judged from inbound server
/// heartbeats and TCP health.
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
    /// Accept an offered forwarded connection by id.
    Accept(String),
}

/// Messages the server sends to the client.
///
/// Serialization and deserialization are hand-written so the type tolerates
/// both bore (always tagged) and spore (sometimes bare) dialects: challenges
/// may arrive as a bare string, `Ack` and `Heartbeat` arrive as bare strings
/// (that is what both real servers actually emit) and are also understood
/// tagged, and unknown fields or variants of the other dialect are ignored.
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
    /// Keepalive; real bore and Spore servers serialize it as the bare
    /// string `"Heartbeat"` roughly every 500 ms.
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
            ServerMessage::Heartbeat => serializer.serialize_str(HEARTBEAT_PAYLOAD),
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
                } else if value == HEARTBEAT_PAYLOAD {
                    Ok(ServerMessage::Heartbeat)
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

    #[test]
    fn max_frame_len_matches_bore() {
        // ekzhang/bore src/shared.rs: MAX_FRAME_LENGTH = 256, codec
        // AnyDelimiterCodec::new_with_max_length(vec![0], vec![0], 256).
        assert_eq!(MAX_FRAME_LEN, 256);
    }

    /// In-memory [`tokio::io::AsyncRead`] source over a byte slice.
    struct SliceSource(std::io::Cursor<Vec<u8>>);

    impl SliceSource {
        fn new(bytes: Vec<u8>) -> Self {
            Self(std::io::Cursor::new(bytes))
        }
    }

    impl tokio::io::AsyncRead for SliceSource {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let mut cursor = &mut self.get_mut().0;
            Poll::Ready(std::io::Read::read(&mut cursor, buf.initialize_unfilled()).map(|n| {
                // SAFETY-free alternative: advance the ReadBuf by n.
                unsafe { buf.assume_init(n) };
                buf.advance(n);
            }))
        }
    }

    #[tokio::test]
    async fn send_writes_exactly_one_frame() {
        let mut sink = VecSink(Vec::new());
        send(&mut sink, &ClientMessage::Hello(7)).await.unwrap();
        assert_eq!(sink.0, encode_frame(&ClientMessage::Hello(7)).unwrap());
        // Explicit wire pin: compact JSON + a single NUL byte.
        assert_eq!(sink.0, b"{\"Hello\":7}\x00");
        let mut reader = FrameReader::new(SliceSource::new(sink.0));
        assert_eq!(
            reader.next_client_message().await.unwrap(),
            Some(ClientMessage::Hello(7))
        );
    }

    #[test]
    fn drain_buffer_returns_complete_and_partial_bytes_then_clears() {
        let frame = encode_frame(&Probe { hello: 9 }).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&frame);
        dec.push(b"raw tail");

        let mut expected = frame.clone();
        expected.extend_from_slice(b"raw tail");
        assert_eq!(dec.drain_buffer(), expected);
        assert!(dec.next_payload().unwrap().is_none());
        assert!(dec.drain_buffer().is_empty());
    }

    #[tokio::test]
    async fn frame_reader_into_parts_hands_back_pending_raw_bytes() {
        let frame = encode_frame(&Probe { hello: 3 }).unwrap();
        let mut chunk = frame.clone();
        chunk.extend_from_slice(b"tail");

        let mut reader = FrameReader::new(SliceSource::new(chunk));
        assert!(reader.next_frame().await.unwrap().is_some());

        let (_stream, mut decoder) = reader.into_parts();
        assert_eq!(decoder.drain_buffer(), b"tail");
    }

    #[tokio::test]
    async fn frame_reader_yields_typed_server_messages_until_eof() {
        let mut sink = VecSink(Vec::new());
        send(&mut sink, &ServerMessage::Hello(42)).await.unwrap();
        write_frame(&mut sink, &serde_json::to_vec("Ack").unwrap())
            .await
            .unwrap();
        send(&mut sink, &ServerMessage::Error("boom".into()))
            .await
            .unwrap();

        let mut reader = FrameReader::new(SliceSource::new(sink.0));
        assert_eq!(
            reader.next_server_message().await.unwrap(),
            Some(ServerMessage::Hello(42))
        );
        assert_eq!(
            reader.next_server_message().await.unwrap(),
            Some(ServerMessage::Ack)
        );
        assert_eq!(
            reader.next_server_message().await.unwrap(),
            Some(ServerMessage::Error("boom".into()))
        );
        assert_eq!(reader.next_server_message().await.unwrap(), None);
    }

    #[tokio::test]
    async fn frame_reader_yields_typed_client_messages() {
        let mut sink = VecSink(Vec::new());
        send(&mut sink, &ClientMessage::Accept("abc".into()))
            .await
            .unwrap();

        let mut reader = FrameReader::new(SliceSource::new(sink.0));
        assert_eq!(
            reader.next_client_message().await.unwrap(),
            Some(ClientMessage::Accept("abc".into()))
        );
        assert_eq!(reader.next_client_message().await.unwrap(), None);
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
    fn encode_frame_appends_single_nul_delimiter() {
        let frame = encode_frame(&Probe { hello: 12345 }).unwrap();
        let mut expected = br#"{"hello":12345}"#.to_vec();
        expected.push(0);
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
    fn decoder_yields_empty_payload_for_leading_delimiter() {
        let mut dec = FrameDecoder::new();
        dec.push(&[0, b'x']);
        assert_eq!(dec.next_payload().unwrap().unwrap(), Vec::<u8>::new());
        // The trailing byte is a new, still-incomplete frame.
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_rejects_oversize_frame_without_delimiter() {
        let mut dec = FrameDecoder::new();
        dec.push(&vec![b'a'; MAX_FRAME_LEN + 1]);
        match dec.next_payload() {
            Err(ProtocolError::FrameTooLarge(n)) => assert_eq!(n, MAX_FRAME_LEN + 1),
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn decoder_tolerates_max_frame_bytes_without_delimiter() {
        // At the limit the frame is still legal: the decoder keeps waiting
        // for the delimiter instead of erroring.
        let mut dec = FrameDecoder::new();
        dec.push(&vec![b'a'; MAX_FRAME_LEN]);
        assert!(dec.next_payload().unwrap().is_none());
    }

    #[test]
    fn decoder_accepts_max_length_payload_with_delimiter() {
        // A payload of exactly MAX_FRAME_LEN bytes plus the delimiter is
        // decodable: bore's codec searches buf[0..max_length+1) for the
        // delimiter, so a delimiter at index max_length is in range.
        let mut wire = vec![b'a'; MAX_FRAME_LEN];
        wire.push(0);
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(
            dec.next_payload().unwrap().unwrap(),
            vec![b'a'; MAX_FRAME_LEN]
        );
        assert!(dec.next_payload().unwrap().is_none());
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
        let wire = vec![b'a'; MAX_FRAME_LEN + 1];
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
    async fn write_frame_appends_single_nul() {
        let mut sink = VecSink(Vec::new());
        write_frame(&mut sink, br#"{"hello":1}"#).await.unwrap();
        let mut expected = br#"{"hello":1}"#.to_vec();
        expected.push(0);
        assert_eq!(sink.0, expected);
    }

    #[tokio::test]
    async fn write_frame_rejects_payload_that_would_overflow_the_peer_limit() {
        // Spore's read_frame rejects buffers over 256 bytes INCLUDING the
        // delimiter, so frames we build may carry at most 255 payload bytes.
        let mut sink = VecSink(Vec::new());
        let big = vec![b'x'; MAX_FRAME_LEN];
        let err = write_frame(&mut sink, &big).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(sink.0.is_empty(), "nothing may hit the wire");
    }

    #[tokio::test]
    async fn write_frame_accepts_payload_at_the_peer_boundary() {
        let mut sink = VecSink(Vec::new());
        write_frame(&mut sink, &vec![b'x'; MAX_FRAME_LEN - 1])
            .await
            .unwrap();
        let mut expected = vec![b'x'; MAX_FRAME_LEN - 1];
        expected.push(0);
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
    fn client_parse_rejects_heartbeat_frames() {
        // Real bore's ClientMessage enum has no Heartbeat variant, so a
        // client must never originate one; the parser enforces that by
        // refusing to decode both spellings.
        assert!(matches!(
            parse_client_message(br#""Heartbeat""#),
            Err(ProtocolError::MalformedJson(_))
        ));
        assert!(matches!(
            parse_client_message(br#"{"Heartbeat":null}"#),
            Err(ProtocolError::MalformedJson(_))
        ));
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
            // Real bore and Spore servers emit the keepalive as the bare
            // string "Heartbeat" (unit variant of the server enum).
            (ServerMessage::Heartbeat, r#""Heartbeat""#),
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
    fn server_parses_heartbeat_bare_and_tagged() {
        // The canonical form real servers put on the wire is the bare
        // string; tagged spellings are tolerated.
        assert_eq!(
            parse_server_message(br#""Heartbeat""#).unwrap(),
            ServerMessage::Heartbeat
        );
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
    fn server_bare_heartbeat_is_not_mistaken_for_a_challenge() {
        // Spore sends the challenge nonce as a bare string too; the literal
        // "Heartbeat" must win over the challenge fallback.
        assert_eq!(
            parse_server_message(br#""Heartbeat""#).unwrap(),
            ServerMessage::Heartbeat
        );
        assert_eq!(
            parse_server_message(br#""550e8400-e29b-41d4-a716-446655440000""#).unwrap(),
            ServerMessage::Challenge("550e8400-e29b-41d4-a716-446655440000".to_string())
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

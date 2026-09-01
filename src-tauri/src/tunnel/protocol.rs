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
}

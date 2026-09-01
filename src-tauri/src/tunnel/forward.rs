//! Per-connection data plane: dial the local service and pipe bytes between it
//! and the tunnel's remote stream, counting traffic in both directions.
//!
//! Direction naming follows the tunnel's point of view:
//!
//! - `up` — traffic arriving from the remote tunnel endpoint and being written
//!   to the local service (remote → local).
//! - `down` — traffic read from the local service and sent to the remote tunnel
//!   endpoint (local → remote).

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::net::TcpStream;

use crate::tunnel::TunnelError;

/// How long [`connect_local`] may take before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Bytes transferred through a tunnel connection, split by direction.
///
/// Shared between the forwarder tasks and the supervisor as
/// `std::sync::Arc<ByteCounters>`; loads and stores use relaxed ordering, which
/// is enough for progress reporting.
#[derive(Debug)]
pub struct ByteCounters {
    /// Bytes flowing remote → local.
    pub up: AtomicU64,
    /// Bytes flowing local → remote.
    pub down: AtomicU64,
}

impl ByteCounters {
    /// Counters starting at zero in both directions.
    pub fn new() -> Self {
        Self {
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
        }
    }

    /// Current `(up, down)` totals.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.up.load(Ordering::Relaxed),
            self.down.load(Ordering::Relaxed),
        )
    }
}

impl Default for ByteCounters {
    fn default() -> Self {
        Self::new()
    }
}

/// Connects to `host:port`, giving up after [`CONNECT_TIMEOUT`].
///
/// A refused connection is reported as [`TunnelError::LocalServiceDown`] so the
/// caller can tell "the local service is not running" apart from generic IO
/// trouble; running past the deadline is reported as
/// [`TunnelError::ConnectTimeout`].
pub async fn connect_local(host: &str, port: u16) -> Result<TcpStream, TunnelError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(err)) if err.kind() == io::ErrorKind::ConnectionRefused => {
            Err(TunnelError::LocalServiceDown {
                host: host.to_string(),
                port,
            })
        }
        Ok(Err(err)) => Err(TunnelError::Io(err)),
        Err(_) => Err(TunnelError::ConnectTimeout {
            addr: format!("{host}:{port}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    async fn bind_loopback() -> (SocketAddr, TcpListener) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        (addr, listener)
    }

    /// An ephemeral port that nothing is listening on any more.
    async fn refused_port() -> u16 {
        let (addr, listener) = bind_loopback().await;
        drop(listener);
        addr.port()
    }

    #[test]
    fn byte_counters_start_at_zero_and_report_both_directions() {
        let counters = ByteCounters::new();
        assert_eq!(counters.snapshot(), (0, 0));

        counters.up.fetch_add(7, Ordering::Relaxed);
        counters.down.fetch_add(3, Ordering::Relaxed);

        assert_eq!(counters.snapshot(), (7, 3));
        assert_eq!(ByteCounters::default().snapshot(), (0, 0));
    }

    #[tokio::test]
    async fn connect_local_maps_a_refused_port_to_local_service_down() {
        let refused = refused_port().await;

        match connect_local("127.0.0.1", refused).await {
            Err(TunnelError::LocalServiceDown { host, port }) => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, refused);
            }
            other => panic!("expected LocalServiceDown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_local_returns_a_stream_for_a_live_listener() {
        let (addr, listener) = bind_loopback().await;

        // Accept concurrently so the connect handshake completes at once.
        let (connected, accepted) =
            tokio::join!(connect_local("127.0.0.1", addr.port()), listener.accept());

        let stream = connected.expect("connecting to a live listener should succeed");
        let (peer, peer_addr) = accepted.expect("accepting should succeed");
        // The accepted peer is the very socket `connect_local` handed us.
        assert_eq!(peer_addr.port(), stream.local_addr().unwrap().port());
        drop(peer);
    }
}

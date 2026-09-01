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
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::tunnel::TunnelError;

/// How long [`connect_local`] may take before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Read granularity of a single pump: counters advance once per chunk so they
/// stay observable while a large transfer is still in flight.
const CHUNK: usize = 8 * 1024;

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

/// Which of the two pump tasks a result came from.
enum Side {
    /// The remote → local pump.
    Up,
    /// The local → remote pump.
    Down,
}

/// Copies bytes in both directions between the remote tunnel stream and the
/// local service stream, counting every chunk in `counters`.
///
/// Follows `tokio::io::copy_bidirectional` semantics: when one direction
/// reaches a clean EOF the write side of that direction is shut down (so the
/// peer sees the half-close) while the opposite direction keeps pumping until
/// it also reaches EOF. Returns `Ok(())` once both directions drained, or the
/// first IO error, which aborts the other direction.
pub async fn forward_bidirectional(
    remote: TcpStream,
    local: TcpStream,
    counters: Arc<ByteCounters>,
) -> Result<(), TunnelError> {
    let (remote_read, remote_write) = tokio::io::split(remote);
    let (local_read, local_write) = tokio::io::split(local);

    // remote → local, counted as `up`.
    let up_counters = Arc::clone(&counters);
    let mut up = tokio::spawn(async move {
        pump(remote_read, local_write, &up_counters.up).await
    });

    // local → remote, counted as `down`.
    let down_counters = Arc::clone(&counters);
    let mut down = tokio::spawn(async move {
        pump(local_read, remote_write, &down_counters.down).await
    });

    // Wait for whichever direction settles first. A clean EOF on one side must
    // not cancel the other — it may still be delivering traffic — so only an
    // error tears the whole connection down.
    let (finished, first) = tokio::select! {
        res = &mut up => (Side::Up, res),
        res = &mut down => (Side::Down, res),
    };

    if let Err(err) = first.map_err(task_failure).and_then(|pumped| pumped.map_err(TunnelError::Io))
    {
        match finished {
            Side::Up => down.abort(),
            Side::Down => up.abort(),
        }
        return Err(err);
    }

    // The first direction drained cleanly; the other one finishes on its own.
    let second = match finished {
        Side::Up => down.await,
        Side::Down => up.await,
    };
    second.map_err(task_failure)??;
    Ok(())
}

/// Runs one forwarded connection end to end.
///
/// The local dial happens first, so an unreachable local service surfaces as
/// [`TunnelError::LocalServiceDown`] before any payload data flows; after that
/// the two streams are piped through [`forward_bidirectional`].
pub async fn run_forwarder(
    remote: TcpStream,
    host: &str,
    port: u16,
    counters: Arc<ByteCounters>,
) -> Result<(), TunnelError> {
    let local = connect_local(host, port).await?;
    forward_bidirectional(remote, local, counters).await
}

/// Copies a single direction in [`CHUNK`]-sized steps, counting each chunk as
/// it is read so the totals stay observable while the transfer is running.
///
/// On a clean EOF (`read` returns `0`) the write half is shut down, propagating
/// the half-close to the peer on the far end of this direction; the opposite
/// direction is unaffected and keeps flowing.
async fn pump<R, W>(mut reader: R, mut writer: W, counter: &AtomicU64) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        // Count before writing, so a supervisor polling the counters sees the
        // bytes as soon as they are read rather than after they are flushed.
        counter.fetch_add(n as u64, Ordering::Relaxed);
        writer.write_all(&chunk[..n]).await?;
    }
}

/// Turns a pump task failure (panic or abort) into a [`TunnelError`].
fn task_failure(err: tokio::task::JoinError) -> TunnelError {
    TunnelError::Io(io::Error::other(err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    /// Upper bound for any asynchronous expectation. Loopback traffic settles
    /// in microseconds; this only turns a broken forwarder into a fast test
    /// failure instead of a hang.
    const DEADLINE: Duration = Duration::from_secs(5);
    /// Poll interval while waiting for an asynchronous expectation.
    const POLL: Duration = Duration::from_millis(10);

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

    /// Polls `predicate` until it holds, panicking once `DEADLINE` elapses.
    async fn wait_until(description: &str, predicate: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while !predicate() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            tokio::time::sleep(POLL).await;
        }
    }

    /// `AsyncReadExt::read_exact` bounded by [`DEADLINE`], so a forwarder that
    /// never delivers the bytes fails the test instead of hanging it.
    async fn read_exact_deadlined(stream: &mut TcpStream, buf: &mut [u8]) -> io::Result<usize> {
        tokio::time::timeout(DEADLINE, stream.read_exact(buf))
            .await
            .expect("read completed within the deadline")
    }

    /// Spawns the stand-in remote endpoint: it accepts one connection, greets
    /// with `READY\n` and then echoes every byte back until it observes EOF.
    ///
    /// The returned task finishes exactly when the remote endpoint has seen
    /// EOF, which lets tests assert that a half-close really propagated.
    async fn spawn_greeting_echo_remote() -> (SocketAddr, tokio::task::JoinHandle<io::Result<()>>) {
        let (addr, listener) = bind_loopback().await;
        let task = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await?;
            conn.write_all(b"READY\n").await?;
            let mut chunk = vec![0u8; 256];
            loop {
                let n = conn.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                conn.write_all(&chunk[..n]).await?;
            }
            Ok(())
        });
        (addr, task)
    }

    /// Binds the stand-in local service; the accepted socket is the peer the
    /// test drives (`P`).
    async fn bind_local_service() -> (SocketAddr, TcpListener) {
        bind_loopback().await
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

    // Wiring under test (echo endpoint == "remote"):
    //
    //   test peer P <-> C [local stream] <-> forwarder <-> R [remote stream] <-> echo
    //
    // Direction mapping: `forward_bidirectional(remote = R, local = C)` counts
    // the echo travelling R -> C -> P as `up`, and P's requests travelling
    // P -> C -> R as `down`. The remote greets with "READY\n" (6 bytes) before
    // echoing anything, so the two totals stay distinct: `up` ends at 6 banner
    // + 10 echoed bytes = 16 while `down` is exactly the 10 bytes P sent, which
    // proves the counters are not swapped.
    #[tokio::test]
    async fn forward_bidirectional_counts_both_directions_end_to_end() {
        let (remote_addr, _remote) = spawn_greeting_echo_remote().await;
        let (local_addr, local_listener) = bind_local_service().await;

        let r = TcpStream::connect(remote_addr).await.unwrap();
        let c = TcpStream::connect(local_addr).await.unwrap();
        let (mut p, _) = local_listener.accept().await.unwrap();

        let counters = Arc::new(ByteCounters::new());
        let task_counters = Arc::clone(&counters);
        let forwarder = tokio::spawn(async move {
            forward_bidirectional(r, c, task_counters).await
        });

        // The remote's greeting must arrive through the forwarder.
        let mut buf = [0u8; 16];
        read_exact_deadlined(&mut p, &mut buf[..6]).await.unwrap();
        assert_eq!(&buf[..6], b"READY\n");

        // Round trip one.
        p.write_all(b"ping\n").await.unwrap();
        read_exact_deadlined(&mut p, &mut buf[..5]).await.unwrap();
        assert_eq!(&buf[..5], b"ping\n");

        // Counters must be observable mid-flight, before the rest of the
        // traffic is driven: banner + first echo counted as `up`, the first
        // request as `down`.
        wait_until("mid-flight byte totals", || counters.snapshot() == (11, 5)).await;

        // Round trip two.
        p.write_all(b"PING\n").await.unwrap();
        read_exact_deadlined(&mut p, &mut buf[..5]).await.unwrap();
        assert_eq!(&buf[..5], b"PING\n");

        wait_until("final byte totals", || counters.snapshot() == (16, 10)).await;
        assert_eq!(counters.snapshot(), (16, 10));
        drop(forwarder);
    }

    /// `read_exact` for `buf.len()` bytes in small steps, bounded by
    /// [`DEADLINE`].
    async fn read_n_deadlined(stream: &mut TcpStream, want: usize) -> io::Result<Vec<u8>> {
        let mut out = vec![0u8; want];
        for step in out.chunks_mut(512) {
            read_exact_deadlined(stream, step).await?;
        }
        Ok(out)
    }

    #[tokio::test]
    async fn forward_bidirectional_counts_across_chunk_boundaries() {
        let (remote_addr, _remote) = spawn_greeting_echo_remote().await;
        let (local_addr, local_listener) = bind_local_service().await;

        let r = TcpStream::connect(remote_addr).await.unwrap();
        let c = TcpStream::connect(local_addr).await.unwrap();
        let (mut p, _) = local_listener.accept().await.unwrap();

        let counters = Arc::new(ByteCounters::new());
        let task_counters = Arc::clone(&counters);
        tokio::spawn(async move {
            forward_bidirectional(r, c, task_counters).await
        });

        let mut buf = [0u8; 6];
        read_exact_deadlined(&mut p, &mut buf).await.unwrap();
        assert_eq!(&buf, b"READY\n");

        // 20_000 bytes span three of the pump's 8 KiB chunks, so the totals
        // prove every chunk was counted, not just the first one.
        let payload: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        p.write_all(&payload).await.unwrap();
        let echoed = read_n_deadlined(&mut p, payload.len()).await.unwrap();
        assert_eq!(echoed, payload, "the echo must survive the round trip");

        let up = 20_000 + b"READY\n".len() as u64;
        let down = payload.len() as u64;
        wait_until("multi-chunk byte totals", || counters.snapshot() == (up, down)).await;
        assert_eq!(counters.snapshot(), (20_006, 20_000));
    }

    #[tokio::test]
    async fn forward_bidirectional_propagates_eof_and_completes_cleanly() {
        let (remote_addr, remote) = spawn_greeting_echo_remote().await;
        let (local_addr, local_listener) = bind_local_service().await;

        let r = TcpStream::connect(remote_addr).await.unwrap();
        let c = TcpStream::connect(local_addr).await.unwrap();
        let (mut p, _) = local_listener.accept().await.unwrap();

        let counters = Arc::new(ByteCounters::new());
        let task_counters = Arc::clone(&counters);
        let forwarder = tokio::spawn(async move {
            forward_bidirectional(r, c, task_counters).await
        });

        // Warm the pipes, then half-close P: it will not send anything else.
        let mut buf = [0u8; 16];
        read_exact_deadlined(&mut p, &mut buf[..6]).await.unwrap();
        p.write_all(b"ping\n").await.unwrap();
        read_exact_deadlined(&mut p, &mut buf[..5]).await.unwrap();

        p.shutdown().await.unwrap();

        // The forwarder keeps pumping the still-open direction, then returns
        // Ok(()) once both sides reached EOF.
        let joined = tokio::time::timeout(DEADLINE, forwarder)
            .await
            .expect("forwarder should finish once both directions reached EOF")
            .expect("forwarder task should not panic");
        joined.expect("a clean EOF on both sides must not surface as an error");

        // The remote endpoint observed the propagated EOF (its echo loop ended).
        tokio::time::timeout(DEADLINE, remote)
            .await
            .expect("remote should observe EOF within the deadline")
            .expect("remote task should not panic")
            .expect("remote echo loop should end cleanly");

        // And P in turn sees EOF once the remote side closed.
        let n = p.read(&mut buf).await.unwrap();
        assert_eq!(n, 0, "P should observe EOF after the remote side closed");
    }

    #[tokio::test]
    async fn run_forwarder_surfaces_a_dead_local_service_before_any_traffic() {
        let refused = refused_port().await;
        let (addr, listener) = bind_loopback().await;

        // A perfectly healthy remote stream: the local preflight must still
        // reject the connection before any payload flows.
        let (remote, _accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
        let remote = remote.unwrap();

        let counters = Arc::new(ByteCounters::new());
        let err = run_forwarder(remote, "127.0.0.1", refused, Arc::clone(&counters))
            .await
            .unwrap_err();

        assert!(matches!(err, TunnelError::LocalServiceDown { .. }));
        assert_eq!(
            counters.snapshot(),
            (0, 0),
            "no payload may flow when the local preflight fails"
        );
    }

    #[tokio::test]
    async fn run_forwarder_pipes_between_remote_and_local_service() {
        // Remote endpoint: sends one request, reads the reply, then half-closes.
        let (remote_addr, remote_listener) = bind_loopback().await;
        let remote = tokio::spawn(async move {
            let (mut conn, _) = remote_listener.accept().await?;
            conn.write_all(b"hi\n").await?;
            let mut buf = [0u8; 6];
            read_exact_deadlined(&mut conn, &mut buf).await?;
            conn.shutdown().await?;
            let n = conn.read(&mut buf).await?;
            assert_eq!(n, 0, "remote should observe EOF once the local side closed");
            Ok::<(), io::Error>(())
        });

        // Local service: answers every request with a fixed "LOCAL\n" frame.
        let (local_addr, local_listener) = bind_loopback().await;
        let local = tokio::spawn(async move {
            let (mut conn, _) = local_listener.accept().await?;
            let mut buf = vec![0u8; 256];
            loop {
                let n = conn.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                conn.write_all(b"LOCAL\n").await?;
            }
            Ok::<(), io::Error>(())
        });

        let remote_stream = TcpStream::connect(remote_addr).await.unwrap();
        let counters = Arc::new(ByteCounters::new());
        let outcome = run_forwarder(
            remote_stream,
            local_addr.ip().to_string().as_str(),
            local_addr.port(),
            Arc::clone(&counters),
        )
        .await;

        outcome.expect("a fully drained exchange should forward without error");
        tokio::time::timeout(DEADLINE, remote)
            .await
            .expect("remote driver should finish within the deadline")
            .expect("remote task should not panic")
            .expect("remote exchange should be clean");
        tokio::time::timeout(DEADLINE, local)
            .await
            .expect("local service should finish within the deadline")
            .expect("local task should not panic")
            .expect("local service should end cleanly");

        // Same direction mapping as above: "hi\n" (3 bytes) came from the
        // remote, "LOCAL\n" (6 bytes) went back to it.
        wait_until("forwarder byte totals", || counters.snapshot() == (3, 6)).await;
        assert_eq!(counters.snapshot(), (3, 6));
    }
}

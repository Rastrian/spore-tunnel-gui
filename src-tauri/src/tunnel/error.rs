//! Shared error type for the tunnel protocol stack.

/// Errors produced while establishing or running a tunnel.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    /// Underlying TCP/IO failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// TCP connection to the given address did not complete in time.
    #[error("could not connect to {addr} within the timeout")]
    ConnectTimeout { addr: String },

    /// Server answered the handshake with an `Error` message.
    #[error("server rejected the tunnel: {0}")]
    ServerRejected(String),

    /// Challenge/response authentication failed.
    #[error("authentication failed: {0}")]
    AuthFailed(String),

    /// Malformed or unexpected wire traffic.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The local service we forward to refused the connection.
    #[error("local service at {host}:{port} is not reachable")]
    LocalServiceDown { host: String, port: u16 },

    /// The control connection was closed or broke.
    #[error("server connection lost: {0}")]
    Disconnected(String),

    /// The server stopped sending keepalives (Heartbeat/Ack) within the
    /// liveness window — dead without a FIN/RST ever reaching us.
    #[error("no acknowledgement from server within the keepalive window")]
    AckTimeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_service_down_message_names_the_target() {
        let err = TunnelError::LocalServiceDown {
            host: "127.0.0.1".to_string(),
            port: 25565,
        };
        assert_eq!(
            err.to_string(),
            "local service at 127.0.0.1:25565 is not reachable"
        );
    }

    #[test]
    fn server_rejected_carries_server_message() {
        let err = TunnelError::ServerRejected("invalid port".to_string());
        assert_eq!(err.to_string(), "server rejected the tunnel: invalid port");
    }
}

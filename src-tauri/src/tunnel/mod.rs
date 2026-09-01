//! Spore Tunnel protocol core.
//!
//! - [`protocol`]: wire framing and message codecs.
//! - [`client`]: control-channel client with dialect negotiation.
//! - [`forward`]: per-connection data forwarder.
//! - [`supervisor`]: owns the control loop, forwarders, reconnects, status.
//! - [`events`]: frozen event contract (payload types + [`events::EventSink`]).
//! - [`manager`]: multi-tunnel manager with per-tunnel event pumps.

pub mod client;
pub mod error;
pub mod events;
pub mod forward;
pub mod manager;
pub mod protocol;
pub mod supervisor;

#[cfg(test)]
pub mod mock_server;

pub use error::TunnelError;

//! Detection of well-known local services for the onboarding wizard.
//!
//! [`detect_local_service`] probes a fixed table of well-known ports on
//! `127.0.0.1` and returns every one that accepts a TCP connection,
//! labelled with a human-readable name. A successful connect is the whole
//! probe: the stream is dropped immediately, nothing is sent or read.
//!
//! Only well-known ports are ever reported — a port without an entry in
//! the name table is skipped, so the wizard never shows noise like an
//! ephemeral OS port that happens to be open.

use std::time::Duration;
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio::time::timeout;

/// Ports probed by [`detect_local_service`], all with a known name.
pub const SCAN_PORTS: [u16; 9] = [25565, 25575, 8123, 3000, 8000, 8080, 5000, 27015, 7777];

/// Per-probe TCP connect timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);

/// A local listener that answered a probe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DetectedService {
    pub port: u16,
    pub name: &'static str,
}

/// Human-readable name of a well-known port (`None` = not reported).
fn well_known_name(port: u16) -> Option<&'static str> {
    match port {
        25565 => Some("Minecraft"),
        25575 => Some("Minecraft RCON"),
        8123 => Some("Dynmap"),
        3000 | 5000 | 8000 | 8080 => Some("Web (dev)"),
        27015 => Some("Source server"),
        7777 => Some("Terraria / Ark"),
        _ => None,
    }
}

/// Probe `ports` on `host` concurrently and return every open, well-known
/// one, sorted by port. Each probe is a bare TCP connect bounded by
/// [`CONNECT_TIMEOUT`]; the stream is dropped on success.
pub async fn scan_ports(host: &str, ports: &[u16]) -> Vec<DetectedService> {
    let mut probes = JoinSet::new();
    for &port in ports {
        let host = host.to_string();
        probes.spawn(async move {
            // Connect success is the whole probe: nothing is sent or read,
            // the stream (if any) is dropped right here.
            let open = matches!(
                timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port))).await,
                Ok(Ok(_))
            );
            (port, open)
        });
    }

    let mut hits = Vec::new();
    while let Some(joined) = probes.join_next().await {
        if let Ok((port, true)) = joined {
            if let Some(name) = well_known_name(port) {
                hits.push(DetectedService { port, name });
            }
        }
    }
    hits.sort_by_key(|service| service.port);
    hits
}

/// Scan the standard [`SCAN_PORTS`] table on `127.0.0.1` (wizard default).
pub async fn detect_local_service() -> Vec<DetectedService> {
    scan_ports("127.0.0.1", &SCAN_PORTS).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Bind a loopback listener on a free WELL-KNOWN port (first entry of
    /// [`SCAN_PORTS`] that binds), so the hit carries a table name. A bare
    /// `:0` bind would yield an ephemeral port, which is skipped by design.
    async fn bind_well_known() -> std::io::Result<(u16, TcpListener)> {
        for &port in SCAN_PORTS.iter() {
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
                return Ok((port, listener));
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "no well-known port is free on this machine",
        ))
    }

    /// A port with nothing behind it: bind `:0`, remember the port, drop.
    async fn free_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    #[tokio::test]
    async fn scan_reports_an_open_well_known_port_and_skips_closed_ones() {
        let (port, listener) = bind_well_known().await.unwrap();
        let closed = free_port().await;
        assert_ne!(port, closed);

        let hits = scan_ports("127.0.0.1", &[port, closed]).await;
        assert_eq!(
            hits,
            vec![DetectedService {
                port,
                name: well_known_name(port).unwrap(),
            }],
            "one exact hit for the open port, nothing for the closed one"
        );
        drop(listener);
    }

    #[test]
    fn well_known_name_covers_the_full_scan_table() {
        let expected: &[(u16, &str)] = &[
            (25565, "Minecraft"),
            (25575, "Minecraft RCON"),
            (8123, "Dynmap"),
            (3000, "Web (dev)"),
            (5000, "Web (dev)"),
            (8000, "Web (dev)"),
            (8080, "Web (dev)"),
            (27015, "Source server"),
            (7777, "Terraria / Ark"),
        ];
        for &(port, name) in expected {
            assert_eq!(well_known_name(port), Some(name), "port {port}");
        }
        // Every port we scan is in the table…
        for &port in SCAN_PORTS.iter() {
            assert!(
                well_known_name(port).is_some(),
                "SCAN_PORTS contains unnamed port {port}"
            );
        }
        // …and only table ports are named.
        assert_eq!(well_known_name(0), None);
        assert_eq!(well_known_name(7835), None);
        assert_eq!(well_known_name(34567), None);
    }

    #[tokio::test]
    async fn open_ports_without_a_well_known_name_are_skipped() {
        // A real listener on an ephemeral port: open, but not a service we
        // can name, so it must not be reported.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let ephemeral = listener.local_addr().unwrap().port();
        assert_eq!(well_known_name(ephemeral), None);

        let hits = scan_ports("127.0.0.1", &[ephemeral, free_port().await]).await;
        assert!(
            hits.is_empty(),
            "unnamed ports must be skipped, got {hits:?}"
        );
        drop(listener);
    }

    #[tokio::test]
    async fn results_are_sorted_by_port_regardless_of_input_order() {
        let mut listeners = Vec::new();
        let mut ports = Vec::new();
        for _ in 0..3 {
            let (port, listener) = bind_well_known().await.unwrap();
            ports.push(port);
            listeners.push(listener);
        }
        ports.sort_unstable();

        let mut scrambled = ports.clone();
        scrambled.reverse();
        scrambled.push(free_port().await);

        let hits = scan_ports("127.0.0.1", &scrambled).await;
        assert_eq!(
            hits.iter().map(|h| h.port).collect::<Vec<_>>(),
            ports,
            "hits must be exactly the open ports, ascending"
        );
        for hit in &hits {
            assert_eq!(hit.name, well_known_name(hit.port).unwrap());
        }
    }

    #[tokio::test]
    async fn detect_local_service_reports_only_well_known_scan_ports() {
        // Guarantee at least one hit is observable; the rest depends on
        // whatever else runs on this machine.
        let (port, listener) = bind_well_known().await.unwrap();

        let hits = detect_local_service().await;
        let ports: Vec<u16> = hits.iter().map(|h| h.port).collect();
        assert!(ports.contains(&port), "open well-known port not found");
        assert!(
            hits.iter().all(|h| SCAN_PORTS.contains(&h.port)),
            "reported {ports:?}, all must be in SCAN_PORTS"
        );
        assert!(
            hits.iter().all(|h| h.name == well_known_name(h.port).unwrap()),
            "names must come from the table"
        );
        let mut sorted = ports.clone();
        sorted.sort_unstable();
        assert_eq!(ports, sorted, "results must be sorted by port");
        drop(listener);
    }

    #[test]
    fn detected_service_serializes_plainly() {
        let service = DetectedService {
            port: 25565,
            name: "Minecraft",
        };
        assert_eq!(
            serde_json::to_value(&service).unwrap(),
            serde_json::json!({ "port": 25565, "name": "Minecraft" })
        );
    }
}

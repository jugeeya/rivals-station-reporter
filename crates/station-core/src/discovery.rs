//! Finding the operator's hub on the LAN, so a station doesn't have to be
//! told its IP by hand.
//!
//! Why a subnet sweep rather than mDNS/Bonjour: the case worth solving is
//! "the two machines can already reach each other, but the human doesn't
//! know the operator's IP". A sweep succeeds in exactly that case and needs
//! nothing new on the hub side — it reuses `/matchlogger/health`, which the
//! hub has always answered. mDNS would add a dependency and a multicast
//! requirement, and multicast is the first thing venue networks filter
//! (guest SSIDs with client isolation, cheap APs), i.e. it fails hardest
//! precisely where a tournament most needs the help.
//!
//! A host counts as a hub only if it answers with our own [`hub::APP_ID`] —
//! "something replied on port 8787" is not good enough to point a station at.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::hub;

/// Per-host request budget. The hub answers `/health` off an in-memory struct
/// with no I/O, so a real hub on a LAN replies in single-digit milliseconds;
/// this is sized for "a host that is up but not us" (which rejects fast) and
/// to keep the whole sweep inside a couple of seconds.
const PROBE_TIMEOUT_MS: u64 = 400;

/// How many hosts are probed at once. A /24 is 254 addresses, so this puts a
/// full sweep at roughly 254/64 * 400ms ~= 1.6s worst case while staying well
/// under any per-process socket limit.
const PROBE_CONCURRENCY: usize = 64;

/// A hub that answered a probe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FoundHub {
    /// The hub's base URL, ready for the station's forwarder to post to.
    pub url: String,
    /// The event this hub is running, if it has one configured. Shown to the
    /// user so an auto-connect is never a silent guess.
    pub slug: Option<String>,
    /// Whether that hub actually has a start.gg token — a hub without one
    /// can collect sets but can't report them.
    pub startgg: bool,
}

/// This machine's own LAN address, by the same interface-picking trick
/// [`hub::lan_ip`] uses (connect a UDP socket nowhere in particular and read
/// back which local address the OS chose; no packets are sent).
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    sock.connect(("10.255.255.255", 1)).ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(_) => None,
    }
}

/// Every host address on `ip`'s /24, excluding the network and broadcast
/// addresses. A /24 is assumed rather than read from the interface because
/// that is what essentially every venue LAN and consumer router hands out,
/// and guessing wider would multiply the sweep time for no practical gain.
pub fn subnet_hosts(ip: Ipv4Addr) -> Vec<Ipv4Addr> {
    let [a, b, c, _] = ip.octets();
    (1u8..=254).map(|d| Ipv4Addr::new(a, b, c, d)).collect()
}

/// Decide whether a `/matchlogger/health` body came from one of our hubs.
///
/// Returns `None` for anything that isn't recognisably this app: a random
/// service on the port, an older hub predating the `app` field, or a
/// half-written/non-JSON response. Being strict here is the point — the
/// caller may auto-connect to whatever this accepts.
pub fn parse_health(body: &str) -> Option<(Option<String>, bool)> {
    let v: Value = serde_json::from_str(body).ok()?;
    if v.get("app").and_then(|a| a.as_str()) != Some(hub::APP_ID) {
        return None;
    }
    let slug = v
        .get("slug")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let startgg = v.get("startgg").and_then(|s| s.as_bool()).unwrap_or(false);
    Some((slug, startgg))
}

/// Probe one host. Separate from the threading so the accept/reject rule can
/// be tested without a network.
fn probe(client: &reqwest::blocking::Client, ip: Ipv4Addr, port: u16) -> Option<FoundHub> {
    let url = format!("http://{ip}:{port}");
    let resp = client
        .get(format!("{url}/matchlogger/health"))
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().ok()?;
    let (slug, startgg) = parse_health(&body)?;
    Some(FoundHub { url, slug, startgg })
}

/// Sweep the local /24 for hubs.
///
/// `skip` is this machine's own address when it is itself running the hub —
/// an operator/both-mode install finding itself and offering to "connect" to
/// itself is just noise.
///
/// Results are sorted by address so repeated scans are stable rather than
/// ordering by whichever thread happened to finish first.
pub fn scan(port: u16, skip: Option<Ipv4Addr>) -> Vec<FoundHub> {
    let Some(local) = local_ipv4() else {
        return Vec::new();
    };
    let hosts: Vec<Ipv4Addr> = subnet_hosts(local)
        .into_iter()
        .filter(|ip| Some(*ip) != skip)
        .collect();

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(PROBE_TIMEOUT_MS))
        // Each probe is a one-shot to a different host; pooling connections
        // across them would only hold sockets open for hosts we're done with.
        .pool_max_idle_per_host(0)
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // A shared queue drained by a fixed set of workers, rather than 254
    // threads: bounds both socket and thread usage regardless of subnet size.
    let queue = Arc::new(Mutex::new(hosts));
    let (tx, rx) = mpsc::channel::<(Ipv4Addr, FoundHub)>();
    let mut workers = Vec::new();
    for _ in 0..PROBE_CONCURRENCY.min(queue.lock().unwrap().len().max(1)) {
        let queue = queue.clone();
        let client = client.clone();
        let tx = tx.clone();
        workers.push(std::thread::spawn(move || loop {
            let Some(ip) = queue.lock().unwrap().pop() else {
                break;
            };
            if let Some(found) = probe(&client, ip, port) {
                // Receiver outlives the workers; a send failure just means
                // the scan was abandoned, so stopping is the right response.
                if tx.send((ip, found)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let mut found: Vec<(Ipv4Addr, FoundHub)> = rx.iter().collect();
    for w in workers {
        let _ = w.join();
    }
    found.sort_by_key(|(ip, _)| *ip);
    found.into_iter().map(|(_, hub)| hub).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health_body(app: &str, slug: &str, startgg: bool) -> String {
        serde_json::json!({"ok": true, "startgg": startgg, "app": app, "slug": slug}).to_string()
    }

    #[test]
    fn subnet_covers_every_host_but_not_network_or_broadcast() {
        let hosts = subnet_hosts(Ipv4Addr::new(192, 168, 1, 42));
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(192, 168, 1, 254));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 0)));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 255)));
    }

    #[test]
    fn subnet_keeps_the_scanners_own_third_octet() {
        let hosts = subnet_hosts(Ipv4Addr::new(10, 0, 7, 3));
        assert!(hosts.contains(&Ipv4Addr::new(10, 0, 7, 250)));
        assert!(!hosts.contains(&Ipv4Addr::new(10, 0, 8, 1)));
    }

    #[test]
    fn accepts_a_real_hub_and_reports_its_event() {
        let body = health_body(hub::APP_ID, "the-hangout-47", true);
        assert_eq!(
            parse_health(&body),
            Some((Some("the-hangout-47".to_string()), true))
        );
    }

    #[test]
    fn accepts_a_hub_with_no_event_configured() {
        // A local-scoreboard-only hub: reachable and ours, just not bound to
        // a bracket. Still a legitimate connect target.
        let body = health_body(hub::APP_ID, "", false);
        assert_eq!(parse_health(&body), Some((None, false)));
    }

    #[test]
    fn rejects_a_stranger_on_the_same_port() {
        // Anything else listening on 8787 must not be offered to the user.
        assert_eq!(parse_health(r#"{"ok":true,"status":"healthy"}"#), None);
        assert_eq!(parse_health(r#"{"app":"some-other-daemon"}"#), None);
    }

    #[test]
    fn rejects_an_older_hub_without_the_app_field() {
        // Pre-discovery hubs answered {"ok":true,"startgg":bool}. They are
        // real hubs, but we can't tell them apart from anything else that
        // returns ok:true, so they don't qualify for auto-connect.
        assert_eq!(parse_health(r#"{"ok":true,"startgg":true}"#), None);
    }

    #[test]
    fn rejects_malformed_bodies() {
        assert_eq!(parse_health(""), None);
        assert_eq!(parse_health("not json at all"), None);
        assert_eq!(parse_health(r#"{"app":"#), None);
    }

    #[test]
    fn blank_slug_is_none_not_empty_string() {
        let body = health_body(hub::APP_ID, "   ", true);
        assert_eq!(parse_health(&body), Some((None, true)));
    }
}

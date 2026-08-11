//! Rivals Station Reporter core — a 1:1 port of the Python station reporter
//! (`jugeeya.github.io/matchlogger/sender/`), organized the same way:
//!
//! * [`save`]        — GVAS stats-save parsing (via `uesave`), `parse_stats`
//! * [`stats`]       — stat diffing, game results, character table, replays
//! * [`set_machine`] — groups games into sets, writes current/live/set files
//! * [`matching`]    — set/entrant matching (port of broker/worker.js logic)
//! * [`startgg`]     — token-authed start.gg client (operator only)
//! * [`hub`]         — LAN hub state machine + `/matchlogger/*` HTTP server
//! * [`forwarder`]   — station-side POSTs to the LAN hub with dedup state
//! * [`tagdb`]       — the public tag database (jugeeya.github.io/tags), cached to disk
//!
//! Design note: records, sets, and snapshots are `serde_json::Value` (dynamic
//! JSON) rather than typed structs. The Python original passes dicts across
//! every boundary and the wire contract is JSON; keeping the same shape makes
//! the port checkable line-by-line and the hub bit-compatible with the
//! Cloudflare broker. Typed structs live only at the edges (config).

pub mod discovery;
pub mod forwarder;
pub mod hub;
pub mod matching;
pub mod save;
pub mod set_machine;
pub mod startgg;
pub mod startgg_web;
pub mod stats;
pub mod tagdb;

/// Seconds since the Unix epoch, as the Python code's `time.time()` int.
pub fn now_sec() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

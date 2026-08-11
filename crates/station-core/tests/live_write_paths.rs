//! End-to-end exercise of every path that WRITES to start.gg, against a real
//! bracket. Ignored by default: it needs a token and a throwaway event, and
//! it advances that event's bracket for real.
//!
//! ```text
//! RSR_LIVE_TOKEN=<token> \
//! RSR_LIVE_SLUG=tournament/<t>/event/<e> \
//!   cargo test -p station-core --test live_write_paths -- --ignored --nocapture
//! ```
//!
//! The bracket must be RESET (every set a `preview_*` placeholder) when this
//! starts — the first thing it checks is that starting a preview set is what
//! brings a bracket to life, which only happens once per reset.
//!
//! Everything here goes through `Hub`, the same object the app's operator
//! mode drives, so what passes here is what the app does.

use serde_json::{json, Value};
use station_core::hub::Hub;

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// A finished 2-0 set as a station would report it, with the two players'
/// in-game tags matching bracket entrant names.
fn finished_set(id: &str, winner: &str, loser: &str) -> Value {
    let now = station_core::now_sec();
    json!({
        "setId": id, "complete": true, "matchCount": 2, "mode": "LOCAL",
        "winnerSlot": 0, "winnerName": winner, "winnerCharacter": "Orc",
        "startEpoch": now - 600, "endEpoch": now,
        "players": [
            {"slot": 0, "name": winner, "character": "Orc", "wins": 2},
            {"slot": 1, "name": loser, "character": "Gal", "wins": 0},
        ],
        "matches": [
            {"index": 1, "players": [
                {"slot": 0, "name": winner, "character": "Orc", "wins": 1},
                {"slot": 1, "name": loser, "character": "Gal", "wins": 0}]},
            {"index": 2, "players": [
                {"slot": 0, "name": winner, "character": "Orc", "wins": 2},
                {"slot": 1, "name": loser, "character": "Gal", "wins": 0}]},
        ],
    })
}

fn sets_now(hub: &Hub, slug: &str) -> Vec<Value> {
    hub.available_sets(slug)
        .map(|v| v["sets"].as_array().cloned().unwrap_or_default())
        .unwrap_or_default()
}

/// start.gg is eventually consistent right after a mutation: a set can be
/// missing from the next list read for a second or two. Poll rather than
/// asserting on the first answer.
fn sets_until(hub: &Hub, slug: &str, want: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..10 {
        let s = sets_now(hub, slug);
        if want(&s) {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    sets_now(hub, slug)
}

fn step(n: &str) {
    println!("\n=== {n} ===");
}

#[test]
#[ignore = "writes to a real start.gg bracket; needs RSR_LIVE_TOKEN + RSR_LIVE_SLUG"]
fn every_write_path_against_a_real_bracket() {
    let (Some(token), Some(slug)) = (env("RSR_LIVE_TOKEN"), env("RSR_LIVE_SLUG")) else {
        panic!("set RSR_LIVE_TOKEN and RSR_LIVE_SLUG");
    };
    let dir = std::env::temp_dir().join(format!("rsr-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = dir.join("hub-state.json").to_string_lossy().into_owned();

    let hub = Hub::new(
        Some(token),
        None,
        None,
        Some(state),
        Some(Box::new(|m: &str| println!("    [hub] {m}"))),
        None,
        None,
    );
    hub.set_event_slug(&slug);
    hub.set_auto_report(station_core::hub::AutoReport { enabled: true });

    // ---- 1. Start Match on a preview set starts the whole bracket --------
    // Only possible once per bracket reset: after this the phase is live and
    // there are no placeholders left. Skipped (not failed) when the bracket
    // is already started, so the rest still runs.
    step("1. Start Match on a preview set");
    let before = sets_now(&hub, &slug);
    let previews = before
        .iter()
        .filter(|s| s["preview"] == json!(true))
        .count();
    println!("  {} set(s), {previews} of them previews", before.len());

    let (real_id, p0, p1) = if previews > 0 {
        let target = before
            .iter()
            .find(|s| {
                s["preview"] == json!(true)
                    && s["entrants"].as_array().is_some_and(|e| e.len() == 2)
            })
            .expect("a preview set with both entrants")
            .clone();
        let preview_id = target["id"].clone();
        let ents = target["entrants"].clone();
        let names = (
            ents[0]["name"].as_str().unwrap().to_string(),
            ents[1]["name"].as_str().unwrap().to_string(),
        );
        println!("  target {preview_id} — {} vs {}", names.0, names.1);

        let out = hub
            .do_start_match(&slug, &preview_id, Some(1), None)
            .expect("starting a preview set");
        let real_id = out["setId"].clone();
        println!("  started: {preview_id} -> {real_id}");
        assert_ne!(real_id, preview_id, "the set gained a real id");

        let after = sets_until(&hub, &slug, |s| s.iter().any(|x| x["id"] == real_id));
        let still = after.iter().filter(|s| s["preview"] == json!(true)).count();
        println!("  previews remaining: {still} (was {previews})");
        assert_eq!(still, 0, "starting one set materialises the phase");
        (real_id, names.0, names.1)
    } else {
        // Already live — use whichever set is playable and un-played.
        let target = before
            .iter()
            .find(|s| {
                s["state"] != json!(3) && s["entrants"].as_array().is_some_and(|e| e.len() == 2)
            })
            .expect("a playable set")
            .clone();
        let id = target["id"].clone();
        let ents = target["entrants"].clone();
        println!("  SKIPPED — bracket already live; using {id}");
        if target["station"].is_null() {
            hub.do_start_match(&slug, &id, Some(1), None)
                .expect("starting an already-real set");
            println!("  started + assigned to station 1");
        }
        (
            id,
            ents[0]["name"].as_str().unwrap().to_string(),
            ents[1]["name"].as_str().unwrap().to_string(),
        )
    };
    println!("  playing: {p0} vs {p1} on set {real_id}");

    // ---- 2. a station finishes it, and auto-report sends it ---------------
    step("2. auto-report a finished set");
    // The station binding is looked up through start.gg's own set list, which
    // lags a mutation by a second or two — and the hub caches that lookup for
    // STATION_CACHE_S. Wait for the assignment to actually be visible before
    // pretending a station finished a set, or the record binds to nothing.
    let seen = sets_until(&hub, &slug, |s| {
        s.iter()
            .any(|x| x["id"] == real_id && x["station"] == json!(1))
    });
    let mine = seen.iter().find(|x| x["id"] == real_id);
    println!(
        "  set visible as: state={} station={}",
        mine.map(|m| m["state"].clone()).unwrap_or(Value::Null),
        mine.map(|m| m["station"].clone()).unwrap_or(Value::Null)
    );
    // ...and past the station-lookup cache, so the bind reads fresh.
    std::thread::sleep(std::time::Duration::from_secs(16));

    hub.handle_current(&slug, 1, Some(&json!({"state": "set_start"})))
        .unwrap();
    let local_id = "live-test-1";
    hub.handle_ingest(&slug, 1, &finished_set(local_id, &p0, &p1))
        .unwrap();
    let rec = hub.get_set(&slug, 1, &json!(local_id)).expect("record");
    println!(
        "  matched={} confidence={} candidate={} blocker={:?}",
        rec["matchedStartggSetId"],
        rec["confidence"],
        rec["candidateWinnerEntrantId"],
        station_core::hub::auto_report_blocker(&rec)
    );
    assert_eq!(
        rec["matchedStartggSetId"], real_id,
        "bound to the set we just started"
    );

    hub.sweep_auto_report(&slug);
    let rec = hub.get_set(&slug, 1, &json!(local_id)).expect("record");
    println!(
        "  status={} reportedBy={}",
        rec["status"], rec["reportedBy"]
    );
    assert_eq!(rec["status"], json!("reported"));
    assert_eq!(rec["reportedBy"], json!("auto"));

    let state = hub.startgg.set_state(&real_id).expect("set state");
    println!("  start.gg says state={state} (3 = completed)");
    assert_eq!(state, json!(3), "start.gg has the result");

    // ---- 3. correct the result, and re-report over it --------------------
    step("3. edit result on a reported set, then re-report");
    let flipped = hub
        .do_override_result(
            &slug,
            1,
            &json!(local_id),
            &json!([
                {"winnerSlot": 1, "chars": [{"slot": 0, "character": "Orc"},
                                            {"slot": 1, "character": "Gal"}]},
                {"winnerSlot": 1, "chars": [{"slot": 0, "character": "Orc"},
                                            {"slot": 1, "character": "Zet"}]},
                {"winnerSlot": 1, "chars": [{"slot": 0, "character": "Orc"},
                                            {"slot": 1, "character": "Zet"}]},
            ]),
        )
        .expect("correcting a reported set");
    println!(
        "  corrected: winner={} score={:?} games={}",
        flipped["set"]["winnerName"],
        flipped["set"]["players"]
            .as_array()
            .map(|ps| ps.iter().map(|p| p["wins"].clone()).collect::<Vec<_>>()),
        flipped["set"]["matchCount"]
    );
    let new_winner = flipped["candidateWinnerEntrantId"].clone();
    assert!(!new_winner.is_null(), "the corrected winner resolved");

    hub.do_rereport(&slug, 1, &json!(local_id), &new_winner)
        .expect("re-reporting over a completed set");

    let after = sets_now(&hub, &slug);
    let next = after.iter().find(|s| {
        s["fullRoundText"]
            .as_str()
            .is_some_and(|t| t.contains("Final"))
    });
    println!("  re-reported; downstream now: {next:?}");
    let state = hub.startgg.set_state(&real_id).expect("set state");
    assert_eq!(state, json!(3), "still completed, with the new winner");

    // ---- 4. report a set nobody pressed Start Match on --------------------
    step("4. report a set nobody started");
    let live = sets_now(&hub, &slug);
    if let Some(next) = live
        .iter()
        .find(|s| s["entrants"].as_array().is_some_and(|e| e.len() == 2) && s["state"] != json!(3))
    {
        let next_id = next["id"].clone();
        let ents = next["entrants"].clone();
        let (q0, q1) = (
            ents[0]["name"].as_str().unwrap().to_string(),
            ents[1]["name"].as_str().unwrap().to_string(),
        );
        println!("  {next_id} — {q0} vs {q1}, state={}", next["state"]);
        // Assign it to station 2 WITHOUT starting it: the case a TO creates
        // by handing out setups and never pressing Start Match.
        hub.do_reassign_destination(&slug, &next_id, Some(2), None)
            .expect("assigning without starting");
        hub.handle_current(&slug, 2, Some(&json!({"state": "set_start"})))
            .unwrap();
        let id2 = "live-test-2";
        hub.handle_ingest(&slug, 2, &finished_set(id2, &q0, &q1))
            .unwrap();
        let rec = hub.get_set(&slug, 2, &json!(id2)).expect("record");
        println!(
            "  needsStartMatch={} reportable={} blocker={:?}",
            rec["needsStartMatch"],
            rec["reportable"],
            station_core::hub::auto_report_blocker(&rec)
        );
        hub.sweep_auto_report(&slug);
        let rec = hub.get_set(&slug, 2, &json!(id2)).expect("record");
        println!(
            "  status={} reportedBy={}",
            rec["status"], rec["reportedBy"]
        );
        assert_eq!(
            rec["status"],
            json!("reported"),
            "a set nobody called still reports"
        );
        let state = hub.startgg.set_state(&next_id).expect("set state");
        assert_eq!(state, json!(3), "start.gg has it");
    } else {
        println!("  (no further playable set — bracket exhausted)");
    }

    step("done");
    println!("  final bracket:");
    for s in sets_now(&hub, &slug) {
        println!(
            "    {:>12} {:24} state={} station={}",
            s["id"].to_string(),
            s["fullRoundText"].as_str().unwrap_or(""),
            s["state"],
            s["station"]
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

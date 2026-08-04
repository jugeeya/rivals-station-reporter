//! Stat diffing + game-result reconstruction — port of the middle of
//! `rivals_stats.py`.
//!
//! The stats save flushes per game, in real time. Diffing two snapshots of the
//! per-character stat maps recovers the winner / loser / characters / per-game
//! stats; the replay filename supplies timestamp + slots + the GameN counter.
//!
//! (`format_set_rows` from the Python module is intentionally not ported: it
//! was display glue for the Tk table / terminal dashboard, and the Vue UI
//! renders straight from the snapshot.)

use std::collections::{BTreeMap, HashMap};

/// Per-character stat maps we diff. Right column = per-match field name.
pub const STAT_FIELDS: [(&str, &str); 14] = [
    ("MatchesByCharacter", "matches"),
    ("WinsByCharacter", "wins"),
    ("LossesByCharacter", "losses"),
    ("KOsByCharacter", "kos"),
    ("DeathsByCharacter", "deaths"),
    ("FallsByCharacter", "falls"),
    ("DamageDealtByCharacter", "damageDealt"),
    ("DamageTakenByCharacter", "damageTaken"),
    ("ParryAttemptsByCharacter", "parryAttempts"),
    ("ParrySuccessesByCharacter", "parrySuccesses"),
    ("GrabAttemptsByCharacter", "grabAttempts"),
    ("GrabSuccessesByCharacter", "grabSuccesses"),
    ("PummelAttemptsByCharacter", "pummelAttempts"),
    ("PummelSuccessesByCharacter", "pummelSuccesses"),
];

/// Categories that only bookkeep (never shown as per-match stats).
pub const BOOKKEEPING: [&str; 3] = ["MatchesByCharacter", "WinsByCharacter", "LossesByCharacter"];

/// Aggregate rows the save keeps alongside real player tags.
pub const SYNTHETIC: [&str; 2] = ["ALL TAGS", "CUM"];

/// The save/replays store characters as the first 3 letters of the
/// ImmutableName (verified against the game pak's Characters/<Name>/ folders).
pub const CHARACTERS: [(&str, &str); 18] = [
    ("Abs", "Absa"),
    ("Cla", "Clairen"),
    ("Eta", "Etalus"),
    ("Fle", "Fleet"),
    ("For", "Forsburn"),
    ("Gal", "Galvan"),
    ("Gou", "Gouie"),
    ("Kra", "Kragg"),
    ("Lar", "La Reina"),
    ("Lox", "Loxodont"),
    ("May", "Maypul"),
    ("Oly", "Olympia"),
    ("Orc", "Orcane"),
    ("Ran", "Ranno"),
    ("Sla", "Slade"),
    ("Wra", "Wrastor"),
    ("Zet", "Zetterburn"),
    ("Random", "Random"),
];

/// 3-letter code -> full character name (falls back to the code).
pub fn char_full(abbrev: &str) -> String {
    CHARACTERS
        .iter()
        .find(|(code, _)| *code == abbrev)
        .map(|(_, full)| (*full).to_string())
        .unwrap_or_else(|| abbrev.to_string())
}

/// EGameModeType from the save. Only LOCAL is someone playing on this machine
/// against someone else on this machine — i.e. a tournament game. ONLINE and
/// RANKED are ladder games that happen to be played at a station between
/// matches; they get logged and shown, but must never reach the bracket.
pub const LOCAL_MODE: &str = "LOCAL";

/// True only for LOCAL sets. An unknown/missing mode is treated as reportable
/// so sets recorded by other producers (with no mode field) keep working.
pub fn is_reportable(mode: Option<&str>) -> bool {
    match mode {
        None => true,
        Some(m) => m.to_uppercase() == LOCAL_MODE,
    }
}

/// Short human label for the console: "" for local, else "online"/"ranked".
pub fn mode_label(mode: Option<&str>) -> String {
    match mode {
        None => String::new(),
        Some(m) if m.to_uppercase() == LOCAL_MODE => String::new(),
        Some(m) => m.to_lowercase(),
    }
}

/// One side of a reconstructed game (winner or loser entry).
#[derive(Debug, Clone, PartialEq)]
pub struct SidePlayer {
    pub tag: String,
    pub char_code: String,
    pub mode: String,
    /// Per-match stats, bookkeeping categories excluded. BTreeMap so emitted
    /// JSON is deterministically ordered.
    pub stats: BTreeMap<String, i64>,
}

/// `to_game_result` output: exactly one winner and/or one loser (1v1 only).
#[derive(Debug, Clone, PartialEq)]
pub struct GameResult {
    pub mode: String,
    pub winners: Vec<SidePlayer>,
    pub losers: Vec<SidePlayer>,
}

/// A parsed replay filename.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub iso: String,
    pub epoch: i64,
    pub game: i64,
    pub players: Vec<ReplayPlayer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayPlayer {
    pub name: String,
    pub char_code: String,
}

/// `{ 'tag|char|mode': {category: delta} }` for every stat that moved.
pub fn diff(
    prev: &HashMap<String, f64>,
    nxt: &HashMap<String, f64>,
) -> HashMap<String, HashMap<String, f64>> {
    let mut buckets: HashMap<String, HashMap<String, f64>> = HashMap::new();
    for (key, val) in nxt {
        let d = val - prev.get(key).copied().unwrap_or(0.0);
        if d == 0.0 {
            continue;
        }
        let parts: Vec<&str> = key.split('|').collect();
        if parts.len() != 4 {
            continue;
        }
        let (tag, ch, mode, cat) = (parts[0], parts[1], parts[2], parts[3]);
        buckets
            .entry(format!("{tag}|{ch}|{mode}"))
            .or_default()
            .insert(cat.to_string(), d);
    }
    buckets
}

/// Playing "Random" double-counts the game under both Random and the rolled
/// character; drop the Random rows whenever the same tag+mode also moved a
/// real character this game.
fn dealias_random(
    items: Vec<(String, HashMap<String, f64>)>,
) -> Vec<(String, HashMap<String, f64>)> {
    let mut real: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (bk, _) in &items {
        let parts: Vec<&str> = bk.split('|').collect();
        let (tag, ch, mode) = (parts[0], parts[1], parts[2]);
        if ch != "Random" {
            real.insert(format!("{tag}|{mode}"));
        }
    }
    items
        .into_iter()
        .filter(|(bk, _)| {
            let parts: Vec<&str> = bk.split('|').collect();
            let (tag, ch, mode) = (parts[0], parts[1], parts[2]);
            !(ch == "Random" && real.contains(&format!("{tag}|{mode}")))
        })
        .collect()
}

/// `{mode, winners: [{tag, char, stats}], losers: [...]}` or None if this save
/// write wasn't a match.
pub fn to_game_result(buckets: HashMap<String, HashMap<String, f64>>) -> Option<GameResult> {
    // Sort for determinism (Python iterated dict order; nothing downstream
    // depends on ordering because a 1v1 has at most one entry per side).
    let mut items: Vec<(String, HashMap<String, f64>)> = buckets.into_iter().collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let rows = dealias_random(items);

    let side = |pred: &dyn Fn(&HashMap<String, f64>) -> bool| -> Vec<SidePlayer> {
        let mut out = Vec::new();
        for (bk, d) in &rows {
            if !pred(d) {
                continue;
            }
            let parts: Vec<&str> = bk.split('|').collect();
            let (tag, ch, mode) = (parts[0], parts[1], parts[2]);
            let mut stats = BTreeMap::new();
            for (cat, field) in STAT_FIELDS {
                if BOOKKEEPING.contains(&cat) {
                    continue;
                }
                if let Some(v) = d.get(cat) {
                    stats.insert(field.to_string(), v.round() as i64);
                }
            }
            out.push(SidePlayer {
                tag: tag.to_string(),
                char_code: ch.to_string(),
                mode: mode.to_string(),
                stats,
            });
        }
        out
    };

    let winners = side(&|d| d.get("WinsByCharacter").copied().unwrap_or(0.0) > 0.0);
    let losers = side(&|d| d.get("LossesByCharacter").copied().unwrap_or(0.0) > 0.0);
    if winners.is_empty() && losers.is_empty() {
        return None;
    }
    // A 1v1 moves exactly one winner and one loser. Anything wider is the game
    // rewriting several tags' stats at once (a cloud sync / bulk save), not a
    // match — recording it invents a "set" with a dozen players in it.
    if winners.len() > 1 || losers.len() > 1 {
        return None;
    }
    let mode = winners
        .first()
        .or(losers.first())
        .map(|p| p.mode.clone())
        .unwrap_or_default();
    Some(GameResult {
        mode,
        winners,
        losers,
    })
}

/// Replay filename -> {epoch, iso, game, players:[{name, char}]}.
/// e.g. `2026-07-23_19-52-44-606-Player1(Fle)-Player2(Zet)-Game1.rpl`
pub fn parse_replay_name(fname: &str) -> Option<Replay> {
    let re = regex::Regex::new(
        r"^(\d{4})-(\d{2})-(\d{2})_(\d{2})-(\d{2})-(\d{2})-(\d+)-(.+)-Game(\d+)\.rpl$",
    )
    .unwrap();
    let m = re.captures(fname)?;
    let mut players = Vec::new();
    let mid = m.get(8).unwrap().as_str();
    let tok_re = regex::Regex::new(r"^(.*)\(([^()]*)\)?$").unwrap();
    for tok in mid.split(")-") {
        if let Some(pm) = tok_re.captures(tok) {
            players.push(ReplayPlayer {
                name: pm.get(1).unwrap().as_str().to_string(),
                char_code: pm.get(2).unwrap().as_str().to_string(),
            });
        }
    }
    // Replay filenames are in LOCAL wall-clock time; map local -> UTC epoch.
    let get = |i: usize| m.get(i).unwrap().as_str().parse::<u32>().unwrap();
    use chrono::TimeZone;
    let epoch = chrono::Local
        .with_ymd_and_hms(get(1) as i32, get(2), get(3), get(4), get(5), get(6))
        .earliest()?
        .timestamp();
    Some(Replay {
        iso: iso_of(epoch),
        epoch,
        game: m.get(9).unwrap().as_str().parse().ok()?,
        players,
    })
}

/// UTC ISO-8601 with a trailing Z, second resolution (Python's `_iso_of`).
pub fn iso_of(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// UTC `YYYYMMDD_HHMMSS` set-id stamp (Python's `_stamp_of`).
pub fn stamp_of(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y%m%d_%H%M%S").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    type StatRow<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, f64)]);

    /// Build the flat `{tag|char|mode|Category: value}` map the save parser
    /// produces — mirror of the Python test's `snap()` helper.
    fn snap(rows: &[StatRow]) -> HashMap<String, f64> {
        let cat: HashMap<&str, &str> = [
            ("matches", "MatchesByCharacter"),
            ("wins", "WinsByCharacter"),
            ("losses", "LossesByCharacter"),
            ("kos", "KOsByCharacter"),
            ("deaths", "DeathsByCharacter"),
            ("damageDealt", "DamageDealtByCharacter"),
            ("damageTaken", "DamageTakenByCharacter"),
            ("grabSuccesses", "GrabSuccessesByCharacter"),
        ]
        .into_iter()
        .collect();
        let mut flat = HashMap::new();
        for (tag, ch, mode, stats) in rows {
            for (k, v) in *stats {
                flat.insert(format!("{tag}|{ch}|{mode}|{}", cat[k]), *v);
            }
        }
        flat
    }

    fn g1() -> HashMap<String, f64> {
        // Game 1 (both Random): KIM rolled Zet & won, JUGZ! rolled Fle & lost.
        snap(&[
            ("KIM", "Zet", "LOCAL", &[("matches", 1.0), ("wins", 1.0)]),
            ("KIM", "Random", "LOCAL", &[("matches", 1.0), ("wins", 1.0)]),
            (
                "JUGZ!",
                "Fle",
                "LOCAL",
                &[("matches", 1.0), ("losses", 1.0)],
            ),
            (
                "JUGZ!",
                "Random",
                "LOCAL",
                &[("matches", 1.0), ("losses", 1.0)],
            ),
        ])
    }

    fn g2() -> HashMap<String, f64> {
        // Game 2 (both Random): JUGZ! rolled Gal & won w/ stats, KIM lost.
        snap(&[
            (
                "JUGZ!",
                "Gal",
                "LOCAL",
                &[
                    ("matches", 1.0),
                    ("wins", 1.0),
                    ("kos", 3.0),
                    ("damageDealt", 83.0),
                    ("grabSuccesses", 3.0),
                ],
            ),
            (
                "JUGZ!",
                "Random",
                "LOCAL",
                &[("matches", 1.0), ("wins", 1.0)],
            ),
            (
                "KIM",
                "Zet",
                "LOCAL",
                &[
                    ("matches", 1.0),
                    ("losses", 1.0),
                    ("deaths", 3.0),
                    ("damageTaken", 83.0),
                ],
            ),
            (
                "KIM",
                "Random",
                "LOCAL",
                &[("matches", 1.0), ("losses", 1.0)],
            ),
        ])
    }

    #[test]
    fn game1_winner_kim_zet_random_dealiased() {
        let r1 = to_game_result(diff(&HashMap::new(), &g1())).unwrap();
        assert_eq!(r1.winners.len(), 1);
        assert_eq!(r1.winners[0].tag, "KIM");
        assert_eq!(r1.winners[0].char_code, "Zet");
    }

    #[test]
    fn game1_loser_jugz_fle() {
        let r1 = to_game_result(diff(&HashMap::new(), &g1())).unwrap();
        assert_eq!(r1.losers[0].char_code, "Fle");
    }

    #[test]
    fn game2_no_random_in_output() {
        let r2 = to_game_result(diff(&HashMap::new(), &g2())).unwrap();
        assert!(r2
            .winners
            .iter()
            .chain(r2.losers.iter())
            .all(|s| s.char_code != "Random"));
    }

    #[test]
    fn game2_winner_carries_real_stats() {
        let r2 = to_game_result(diff(&HashMap::new(), &g2())).unwrap();
        assert_eq!(r2.winners[0].stats.get("kos"), Some(&3));
        assert_eq!(r2.winners[0].stats.get("damageDealt"), Some(&83));
    }

    #[test]
    fn bookkeeping_fields_excluded_from_per_match_stats() {
        let r2 = to_game_result(diff(&HashMap::new(), &g2())).unwrap();
        assert!(!r2.winners[0].stats.contains_key("losses"));
        assert!(!r2.winners[0].stats.contains_key("matches"));
    }

    #[test]
    fn bulk_rewrite_is_not_a_match() {
        // Several tags moving at once = cloud sync / bulk save, not a game.
        let mut both = g1();
        both.extend(snap(&[
            ("A", "Cla", "LOCAL", &[("wins", 1.0)]),
            ("B", "Kra", "LOCAL", &[("wins", 1.0)]),
        ]));
        assert!(to_game_result(diff(&HashMap::new(), &both)).is_none());
    }

    #[test]
    fn replay_names_parse() {
        let rep1 = parse_replay_name("2026-07-23_19-52-44-606-Player1(Fle)-Player2(Zet)-Game1.rpl")
            .unwrap();
        let rep2 = parse_replay_name("2026-07-23_19-56-58-454-Player1(Gal)-Player2(Zet)-Game2.rpl")
            .unwrap();
        assert_eq!((rep1.game, rep2.game), (1, 2));
        assert_eq!(rep1.players[0].char_code, "Fle");
        assert_eq!(rep1.players[1].char_code, "Zet");
    }

    #[test]
    fn mode_gating() {
        assert!(is_reportable(None));
        assert!(is_reportable(Some("LOCAL")));
        assert!(is_reportable(Some("local")));
        assert!(!is_reportable(Some("ONLINE")));
        assert!(!is_reportable(Some("RANKED")));
        assert_eq!(mode_label(Some("LOCAL")), "");
        assert_eq!(mode_label(Some("ONLINE")), "online");
        assert_eq!(mode_label(None), "");
    }

    #[test]
    fn char_table() {
        assert_eq!(char_full("Cla"), "Clairen");
        assert_eq!(char_full("Lar"), "La Reina");
        assert_eq!(char_full("Random"), "Random");
        assert_eq!(char_full("Clairen"), "Clairen"); // idempotent on full names
    }
}

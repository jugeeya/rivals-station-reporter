//! Turning a flat list of sets into something shaped like a bracket.
//!
//! start.gg hands back sets in no particular order, each carrying a signed
//! `round` and an `identifier`. That's enough to rebuild the tree:
//!
//!   * `round > 0` is the winners side, `round < 0` the losers side, and the
//!     magnitude is the column within that side (winners round 1, 2, 3…).
//!     Grand Final (and its reset) are just the last winners rounds.
//!   * `identifier` orders sets top-to-bottom within a round. It's an
//!     Excel-style column label — A…Z, then AA, AB — so it has to be compared
//!     by length first: plain lexicographic sorting puts "AA" before "Z".
//!
//! All pure, so the column maths is testable without a window or a network.

use std::collections::HashMap;
use std::fmt;

use super::fetch::{Bracket, BracketSet};

/// One phase group (a pool, or the single bracket of a small event). Events
/// with pools into a top-8 have several; the screen shows one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupKey {
    pub id: String,
    pub phase_order: i64,
    pub phase_name: String,
    /// `displayIdentifier` — "1" for a lone bracket, "A"/"B"/… for pools.
    pub label: String,
    /// True when this phase has only one group, so the group label carries no
    /// information and naming it "Bracket 1" would just be noise.
    lone_group_in_phase: bool,
}

impl fmt::Display for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = if self.phase_name.is_empty() {
            "Bracket"
        } else {
            &self.phase_name
        };
        if self.lone_group_in_phase || self.label.is_empty() {
            f.write_str(phase)
        } else {
            write!(f, "{phase} {}", self.label)
        }
    }
}

/// A set, and where it sits vertically in its column.
///
/// `row` is in ROWS, not pixels: one row is the height of a card plus the gap
/// under it. Fractional values are the point — a set fed by two others sits
/// at the average of their rows, i.e. exactly opposite the gap between them,
/// which is what makes a bracket look like a bracket.
#[derive(Debug, Clone)]
pub struct Placed<'a> {
    pub set: &'a BracketSet,
    pub row: f32,
}

/// A round: one vertical column of the drawn bracket.
#[derive(Debug, Clone)]
pub struct Column<'a> {
    /// Signed round, straight from start.gg.
    pub round: i64,
    /// `fullRoundText` ("Winners Quarter-Final"), taken from the first set.
    pub title: &'a str,
    pub sets: Vec<Placed<'a>>,
}

impl Column<'_> {
    /// Row just past the last card — the column's height in rows.
    fn extent(&self) -> f32 {
        self.sets.last().map(|p| p.row + 1.0).unwrap_or(0.0)
    }
}

/// One feeder-to-successor link, in (column index, row) coordinates on one
/// side. The screen turns these into elbows; keeping them unitless here means
/// the geometry is testable without a renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub from_col: usize,
    pub from_row: f32,
    pub to_col: usize,
    pub to_row: f32,
}

/// One side of a group: its columns, plus the links between them.
#[derive(Debug, Clone, Default)]
pub struct Side<'a> {
    pub columns: Vec<Column<'a>>,
    pub links: Vec<Link>,
}

impl Side<'_> {
    /// Tallest column, in rows — the height the whole side needs.
    pub fn rows(&self) -> f32 {
        self.columns.iter().map(Column::extent).fold(0.0, f32::max)
    }
}

/// One group's two sides, each already ordered left-to-right.
#[derive(Debug, Clone, Default)]
pub struct Group<'a> {
    pub winners: Side<'a>,
    pub losers: Side<'a>,
}

impl Group<'_> {
    pub fn is_empty(&self) -> bool {
        self.winners.columns.is_empty() && self.losers.columns.is_empty()
    }
}

/// Sort key for a set label. `(len, text)` so A < Z < AA < AB, which is how
/// start.gg numbers sets once a bracket runs past 26 of them. Non-alphabetic
/// identifiers (some events use plain numbers) still order sensibly because
/// the length comparison handles their digit count first.
fn identifier_key(identifier: &str) -> (usize, String) {
    (identifier.len(), identifier.to_ascii_uppercase())
}

/// Every phase group in the bracket, in play order: phases by `phaseOrder`,
/// then groups by label. The screen's group picker renders this directly.
pub fn groups_of(bracket: &Bracket) -> Vec<GroupKey> {
    let mut keys: Vec<GroupKey> = Vec::new();
    for set in &bracket.sets {
        if keys.iter().any(|k| k.id == set.phase_group_id) {
            continue;
        }
        keys.push(GroupKey {
            id: set.phase_group_id.clone(),
            phase_order: set.phase_order,
            phase_name: set.phase_name.clone(),
            label: set.phase_group_label.clone(),
            lone_group_in_phase: false,
        });
    }
    keys.sort_by(|a, b| {
        a.phase_order
            .cmp(&b.phase_order)
            .then_with(|| identifier_key(&a.label).cmp(&identifier_key(&b.label)))
    });
    // Only now is it knowable whether a phase has one group or many, which is
    // what decides if the label is worth showing at all.
    for i in 0..keys.len() {
        let order = keys[i].phase_order;
        keys[i].lone_group_in_phase = keys.iter().filter(|k| k.phase_order == order).count() == 1;
    }
    keys
}

/// Arrange one phase group's sets into winners and losers columns.
///
/// Sets with `round == 0` (start.gg leaves it unset for a few bracket types
/// that aren't laid out as a tree at all, e.g. round robin) are left out —
/// there's no column they belong in, and guessing one would draw a bracket
/// that isn't the one being played.
pub fn lay_out<'a>(bracket: &'a Bracket, group_id: &str) -> Group<'a> {
    let mut winners: Vec<Vec<&'a BracketSet>> = Vec::new();
    let mut losers: Vec<Vec<&'a BracketSet>> = Vec::new();
    let mut w_rounds: Vec<i64> = Vec::new();
    let mut l_rounds: Vec<i64> = Vec::new();

    for set in &bracket.sets {
        if set.phase_group_id != group_id || set.round == 0 {
            continue;
        }
        let (side, rounds) = if set.round > 0 {
            (&mut winners, &mut w_rounds)
        } else {
            (&mut losers, &mut l_rounds)
        };
        match rounds.iter().position(|r| *r == set.round) {
            Some(i) => side[i].push(set),
            None => {
                rounds.push(set.round);
                side.push(vec![set]);
            }
        }
    }

    Group {
        winners: place_side(winners, w_rounds),
        losers: place_side(losers, l_rounds),
    }
}

/// Give every set on one side a row, and record the links between them.
///
/// A bracket reads correctly only when each set sits level with the sets that
/// feed it, so placement follows the real feeder graph (`prereqId`) rather
/// than any assumption about round sizes. Three things make that harder than
/// walking left to right:
///
///   * **The first round is not always the widest.** With byes, Round 1 is a
///     small pre-round and Round 2 is the full one — Hangout #4 has 4 sets
///     then 8. Anchoring on the leftmost column leaves most of Round 2 with
///     nothing to line up against, and the column comes out shuffled. So the
///     WIDEST column is the anchor, laid out one set per row; everything to
///     its right follows its feeders, and everything to its LEFT follows the
///     sets it feeds into.
///   * **Byes leave dangling feeders.** A bracket with byes names feeder sets
///     start.gg never returns, so a link can point at nothing. A set with one
///     resolvable relative lines up with it; one with none keeps its place in
///     the column.
///   * **Cross-side feeders.** Losers-round sets are fed by the WINNERS side.
///     Those rows mean nothing in this side's coordinates, so only same-side
///     relatives are followed — otherwise the losers bracket gets dragged
///     into the shape of the winners one.
fn place_side<'a>(columns: Vec<Vec<&'a BracketSet>>, rounds: Vec<i64>) -> Side<'a> {
    // Left-to-right on |round|: winners count up, losers count down.
    let mut order: Vec<usize> = (0..columns.len()).collect();
    order.sort_by_key(|&i| rounds[i].abs());
    let cols: Vec<Vec<&'a BracketSet>> = order
        .iter()
        .map(|&i| {
            let mut v = columns[i].clone();
            v.sort_by_key(|s| identifier_key(&s.identifier));
            v
        })
        .collect();
    if cols.is_empty() {
        return Side::default();
    }

    // Which sets are on this side at all, and where each one lives.
    let mut col_of: HashMap<&str, usize> = HashMap::new();
    for (c, col) in cols.iter().enumerate() {
        for set in col {
            col_of.insert(set.id.as_str(), c);
        }
    }
    // Reverse of the feeder graph: set -> the sets it feeds into.
    let mut feeds_into: HashMap<&str, Vec<&str>> = HashMap::new();
    for col in &cols {
        for set in col {
            for feeder in same_side_feeders(set, &col_of) {
                feeds_into.entry(feeder).or_default().push(set.id.as_str());
            }
        }
    }

    // Anchor on the widest column (leftmost of equals), one set per row.
    let anchor = (0..cols.len())
        .max_by_key(|&i| (cols[i].len(), std::cmp::Reverse(i)))
        .expect("cols is non-empty");
    let mut rows: HashMap<&str, f32> = HashMap::new();
    for (i, set) in cols[anchor].iter().enumerate() {
        rows.insert(set.id.as_str(), i as f32);
    }

    // Rightwards: follow the feeders. Leftwards: follow what each set feeds.
    for c in anchor + 1..cols.len() {
        let wanted: Vec<f32> = cols[c]
            .iter()
            .enumerate()
            .map(|(i, set)| {
                mean_of(same_side_feeders(set, &col_of).into_iter(), &rows).unwrap_or(i as f32)
            })
            .collect();
        settle(&cols[c], &wanted, &mut rows);
    }
    for c in (0..anchor).rev() {
        let wanted: Vec<f32> = cols[c]
            .iter()
            .enumerate()
            .map(|(i, set)| {
                let successors = feeds_into.get(set.id.as_str()).cloned().unwrap_or_default();
                mean_of(successors.into_iter(), &rows).unwrap_or(i as f32)
            })
            .collect();
        settle(&cols[c], &wanted, &mut rows);
    }

    // Everything has a row now; build the columns and the lines between them.
    let mut out: Vec<Column<'a>> = Vec::with_capacity(cols.len());
    let mut links: Vec<Link> = Vec::new();
    for (c, col) in cols.iter().enumerate() {
        let mut placed: Vec<Placed<'a>> = col
            .iter()
            .map(|set| Placed {
                set,
                row: rows.get(set.id.as_str()).copied().unwrap_or(0.0),
            })
            .collect();
        placed.sort_by(|a, b| {
            a.row
                .partial_cmp(&b.row)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for p in &placed {
            for feeder in same_side_feeders(p.set, &col_of) {
                let (Some(&from_col), Some(&from_row)) = (col_of.get(feeder), rows.get(feeder))
                else {
                    continue;
                };
                if from_col < c {
                    links.push(Link {
                        from_col,
                        from_row,
                        to_col: c,
                        to_row: p.row,
                    });
                }
            }
        }

        out.push(Column {
            round: rounds[order[c]],
            // The round's name comes from whichever set is topmost, so it has
            // to be read after placement rather than at insertion.
            title: placed
                .first()
                .map(|p| p.set.full_round_text.as_str())
                .unwrap_or(""),
            sets: placed,
        });
    }

    Side {
        columns: out,
        links,
    }
}

/// The sets feeding this one that are actually on this side and were actually
/// returned by start.gg. Both filters matter: a losers set names winners sets,
/// and a bye names sets that don't exist.
fn same_side_feeders<'s>(set: &'s BracketSet, col_of: &HashMap<&str, usize>) -> Vec<&'s str> {
    set.slots
        .iter()
        .filter_map(|s| s.prereq_set_id.as_deref())
        .filter(|id| col_of.contains_key(id))
        .collect()
}

/// Average row of whichever of these sets already has one, or `None` if none
/// of them do.
fn mean_of<'i>(ids: impl Iterator<Item = &'i str>, rows: &HashMap<&str, f32>) -> Option<f32> {
    let known: Vec<f32> = ids.filter_map(|id| rows.get(id).copied()).collect();
    (!known.is_empty()).then(|| known.iter().sum::<f32>() / known.len() as f32)
}

/// Commit one column's wanted rows, pushing apart anything that asked for the
/// same place. Order follows the wanted rows, with the set label breaking
/// ties, so the column stays in a sensible reading order either way.
fn settle<'a>(sets: &[&'a BracketSet], wanted: &[f32], rows: &mut HashMap<&'a str, f32>) {
    let mut pairs: Vec<(f32, &'a BracketSet)> =
        wanted.iter().copied().zip(sets.iter().copied()).collect();
    pairs.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| identifier_key(&a.1.identifier).cmp(&identifier_key(&b.1.identifier)))
    });
    let mut cursor = f32::NEG_INFINITY;
    for (row, set) in pairs {
        let row = row.max(cursor);
        cursor = row + 1.0;
        rows.insert(set.id.as_str(), row);
    }
}

/// The set a TO most likely wants next: the longest-running ongoing set, or
/// failing that the first one that's ready to be called. Used to park the
/// scroll position somewhere useful instead of at round 1 of a finished side.
pub fn set_needing_attention<'a>(bracket: &'a Bracket, group_id: &str) -> Option<&'a BracketSet> {
    let mine = || {
        bracket
            .sets
            .iter()
            .filter(move |s| s.phase_group_id == group_id)
    };
    mine()
        .filter(|s| s.is_ongoing())
        .min_by_key(|s| s.started_at.unwrap_or(i64::MAX))
        .or_else(|| {
            mine()
                .filter(|s| !s.is_complete() && !s.is_ongoing() && s.is_ready() && !s.preview)
                .min_by_key(|s| (s.round.abs(), identifier_key(&s.identifier)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bracket::fetch::{Slot, STATE_COMPLETED, STATE_CREATED, STATE_ONGOING};

    fn set(round: i64, identifier: &str, group: &str, phase_order: i64) -> BracketSet {
        BracketSet {
            id: format!("{group}-{identifier}"),
            preview: false,
            state: STATE_CREATED,
            round,
            identifier: identifier.into(),
            full_round_text: format!("Round {round}"),
            winner_id: None,
            total_games: None,
            started_at: None,
            completed_at: None,
            station: None,
            stream: None,
            phase_group_id: group.into(),
            phase_group_label: group.into(),
            phase_name: "Bracket".into(),
            phase_order,
            bracket_type: "DOUBLE_ELIMINATION".into(),
            slots: [Slot::default(), Slot::default()],
        }
    }

    fn bracket(sets: Vec<BracketSet>) -> Bracket {
        Bracket {
            stations: Vec::new(),
            streams: Vec::new(),
            event_name: "e".into(),
            tournament_name: "t".into(),
            sets,
        }
    }

    /// Say which sets feed this one, by their identifiers within `group`.
    fn fed_by(mut s: BracketSet, group: &str, feeders: [Option<&str>; 2]) -> BracketSet {
        for (slot, feeder) in s.slots.iter_mut().zip(feeders) {
            slot.prereq_set_id = feeder.map(|f| format!("{group}-{f}"));
        }
        s
    }

    fn rows<'a>(col: &'a Column<'a>) -> Vec<(&'a str, f32)> {
        col.sets
            .iter()
            .map(|p| (p.set.identifier.as_str(), p.row))
            .collect()
    }

    #[test]
    fn a_set_sits_opposite_the_gap_between_its_two_feeders() {
        // The shape that makes a bracket readable: A at row 0, B at row 1,
        // and the Quarter-Final they feed centred at row 0.5.
        let b = bracket(vec![
            set(1, "A", "g", 1),
            set(1, "B", "g", 1),
            fed_by(set(2, "M", "g", 1), "g", [Some("A"), Some("B")]),
        ]);
        let g = lay_out(&b, "g");
        assert_eq!(rows(&g.winners.columns[0]), vec![("A", 0.0), ("B", 1.0)]);
        assert_eq!(rows(&g.winners.columns[1]), vec![("M", 0.5)]);

        // And both feeder lines are recorded, so the screen can draw them.
        assert_eq!(
            g.winners.links,
            vec![
                Link {
                    from_col: 0,
                    from_row: 0.0,
                    to_col: 1,
                    to_row: 0.5
                },
                Link {
                    from_col: 0,
                    from_row: 1.0,
                    to_col: 1,
                    to_row: 0.5
                },
            ]
        );
    }

    #[test]
    fn a_full_round_of_pairs_stays_paired_all_the_way_up() {
        let b = bracket(vec![
            set(1, "A", "g", 1),
            set(1, "B", "g", 1),
            set(1, "C", "g", 1),
            set(1, "D", "g", 1),
            fed_by(set(2, "E", "g", 1), "g", [Some("A"), Some("B")]),
            fed_by(set(2, "F", "g", 1), "g", [Some("C"), Some("D")]),
            fed_by(set(3, "G", "g", 1), "g", [Some("E"), Some("F")]),
        ]);
        let g = lay_out(&b, "g");
        assert_eq!(rows(&g.winners.columns[1]), vec![("E", 0.5), ("F", 2.5)]);
        assert_eq!(
            rows(&g.winners.columns[2]),
            vec![("G", 1.5)],
            "the final sits opposite the middle of the whole bracket"
        );
        assert_eq!(g.winners.rows(), 4.0, "four rows tall, from round 1");
    }

    #[test]
    fn a_bye_bracket_anchors_on_the_full_round_not_the_pre_round() {
        // Hangout #4's real shape: Round 1 has 4 sets, Round 2 has 8, because
        // most entrants had byes. Anchoring on Round 1 leaves the rest of
        // Round 2 with nothing to line up against and shuffles the column, so
        // the WIDEST round is the anchor and Round 1 is placed against it.
        let mut sets = vec![
            set(1, "A", "g", 1),
            set(1, "B", "g", 1),
            set(1, "C", "g", 1),
            set(1, "D", "g", 1),
        ];
        // Round 2: E..L, each with one bye seat (a feeder start.gg never
        // returned); four of them are also fed by a real Round 1 set.
        let from = ["A", "-", "B", "-", "C", "-", "D", "-"];
        for (i, ident) in ["E", "F", "G", "H", "I", "J", "K", "L"].iter().enumerate() {
            let mut s = set(2, ident, "g", 1);
            s.slots[0].prereq_set_id = Some("g-BYE".into());
            if from[i] != "-" {
                s.slots[1].prereq_set_id = Some(format!("g-{}", from[i]));
            }
            sets.push(s);
        }
        let b = bracket(sets);
        let g = lay_out(&b, "g");

        assert_eq!(
            rows(&g.winners.columns[1])
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec!["E", "F", "G", "H", "I", "J", "K", "L"],
            "the full round keeps its own order, one set per row"
        );
        assert_eq!(
            rows(&g.winners.columns[0]),
            vec![("A", 0.0), ("B", 2.0), ("C", 4.0), ("D", 6.0)],
            "each pre-round set sits level with the set it feeds"
        );
    }

    #[test]
    fn a_bye_leaves_a_dangling_feeder_and_the_set_follows_the_one_it_has() {
        // Real brackets with byes name feeder sets start.gg never returns —
        // Hangout #4's Winners Round 2 does exactly this. The set must still
        // place, against whichever feeder does exist.
        let mut m = fed_by(set(2, "M", "g", 1), "g", [Some("A"), None]);
        m.slots[1].prereq_set_id = Some("g-GHOST".into());
        let b = bracket(vec![set(1, "A", "g", 1), set(1, "B", "g", 1), m]);
        let g = lay_out(&b, "g");
        assert_eq!(rows(&g.winners.columns[1]), vec![("M", 0.0)]);
        assert_eq!(
            g.winners.links,
            vec![Link {
                from_col: 0,
                from_row: 0.0,
                to_col: 1,
                to_row: 0.0
            }],
            "no line is drawn to a set that was never returned"
        );
    }

    #[test]
    fn a_losers_set_is_not_dragged_into_the_winners_shape() {
        // Losers rounds are fed by the WINNERS side; those rows mean nothing
        // in losers coordinates, so they must be ignored for placement.
        let b = bracket(vec![
            set(1, "A", "g", 1),
            set(1, "B", "g", 1),
            set(1, "C", "g", 1),
            fed_by(set(-1, "X", "g", 1), "g", [Some("B"), Some("C")]),
            fed_by(set(-1, "Y", "g", 1), "g", [Some("A"), None]),
        ]);
        let g = lay_out(&b, "g");
        assert_eq!(
            rows(&g.losers.columns[0]),
            vec![("X", 0.0), ("Y", 1.0)],
            "the first losers column lays out on its own, one per row"
        );
        assert!(
            g.losers.links.is_empty(),
            "no cross-side lines: the winners feeders live in the other canvas"
        );
    }

    #[test]
    fn sets_wanting_the_same_row_are_pushed_apart_in_order() {
        // Two sets both fed only by the same surviving feeder would land on
        // top of each other; the second gets nudged down a row.
        let b = bracket(vec![
            set(1, "A", "g", 1),
            fed_by(set(2, "M", "g", 1), "g", [Some("A"), None]),
            fed_by(set(2, "N", "g", 1), "g", [Some("A"), None]),
        ]);
        let g = lay_out(&b, "g");
        assert_eq!(rows(&g.winners.columns[1]), vec![("M", 0.0), ("N", 1.0)]);
    }

    #[test]
    fn splits_sides_and_orders_columns_outward() {
        let b = bracket(vec![
            set(-5, "AA", "g", 1),
            set(3, "M", "g", 1),
            set(1, "A", "g", 1),
            set(-4, "V", "g", 1),
        ]);
        let g = lay_out(&b, "g");
        assert_eq!(
            g.winners
                .columns
                .iter()
                .map(|c| c.round)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(
            g.losers.columns.iter().map(|c| c.round).collect::<Vec<_>>(),
            vec![-4, -5],
            "losers columns run outward on |round|, not numerically"
        );
    }

    #[test]
    fn set_labels_sort_excel_style_not_lexicographically() {
        // The real Hangout #4 losers round 2 is exactly this: Z alongside
        // AA/AB/AC. Plain string sorting would hoist the AAs above Z.
        let b = bracket(vec![
            set(-5, "AB", "g", 1),
            set(-5, "Z", "g", 1),
            set(-5, "AC", "g", 1),
            set(-5, "AA", "g", 1),
        ]);
        let g = lay_out(&b, "g");
        let order: Vec<&str> = g.losers.columns[0]
            .sets
            .iter()
            .map(|p| p.set.identifier.as_str())
            .collect();
        assert_eq!(order, vec!["Z", "AA", "AB", "AC"]);
    }

    #[test]
    fn groups_are_kept_apart_and_ordered_by_phase() {
        let b = bracket(vec![
            {
                let mut s = set(1, "A", "top8", 2);
                s.phase_name = "Top 8".into();
                s.phase_group_label = "1".into();
                s
            },
            {
                let mut s = set(1, "A", "poolB", 1);
                s.phase_name = "Pools".into();
                s.phase_group_label = "B".into();
                s
            },
            {
                let mut s = set(1, "A", "poolA", 1);
                s.phase_name = "Pools".into();
                s.phase_group_label = "A".into();
                s
            },
        ]);
        let keys = groups_of(&b);
        assert_eq!(
            keys.iter().map(|k| k.id.as_str()).collect::<Vec<_>>(),
            vec!["poolA", "poolB", "top8"]
        );
        // A phase with several groups needs its label; a lone one doesn't.
        assert_eq!(keys[0].to_string(), "Pools A");
        assert_eq!(keys[2].to_string(), "Top 8");

        let g = lay_out(&b, "poolA");
        assert_eq!(g.winners.columns.len(), 1);
        assert_eq!(g.winners.columns[0].sets.len(), 1, "other groups stay out");
    }

    #[test]
    fn roundless_sets_are_left_out_rather_than_guessed_into_a_column() {
        let b = bracket(vec![set(0, "A", "g", 1), set(1, "B", "g", 1)]);
        let g = lay_out(&b, "g");
        assert_eq!(g.winners.columns.len(), 1);
        assert_eq!(g.winners.columns[0].round, 1);
        assert!(g.losers.columns.is_empty());
    }

    #[test]
    fn attention_prefers_the_longest_running_set_then_the_earliest_callable() {
        let mut early = set(1, "A", "g", 1);
        early.state = STATE_ONGOING;
        early.started_at = Some(100);
        let mut late = set(2, "E", "g", 1);
        late.state = STATE_ONGOING;
        late.started_at = Some(500);
        let b = bracket(vec![late, early]);
        assert_eq!(
            set_needing_attention(&b, "g").map(|s| s.identifier.as_str()),
            Some("A"),
            "the set that has been going longest is the one to look at"
        );

        // Nothing playing: fall through to the first set that could be called.
        let mut done = set(1, "A", "g", 1);
        done.state = STATE_COMPLETED;
        let mut ready = set(2, "E", "g", 1);
        ready.slots[0].entrant_id = Some("1".into());
        ready.slots[1].entrant_id = Some("2".into());
        let mut waiting = set(3, "M", "g", 1);
        waiting.slots[0].entrant_id = Some("1".into());
        let b = bracket(vec![done, waiting, ready]);
        assert_eq!(
            set_needing_attention(&b, "g").map(|s| s.identifier.as_str()),
            Some("E"),
            "a half-seeded set is not callable"
        );
    }
}

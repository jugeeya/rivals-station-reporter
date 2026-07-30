// Pure formatting/derivation helpers for the operator console's per-set rows.
// Kept out of OperatorSetRow.vue so its template stays readable.
//
// Never invents data: every read here traces back to a field `summarize_set`
// or `derive_games` (crates/station-core/src/matching.rs) actually puts on
// `record.set`, or that `record_for` (crates/station-core/src/hub.rs) puts on
// the record itself.

import type { HubRecord } from '../types';

// The hub's records are loosely-typed JSON (see HubRecord in types.ts); these
// narrower shapes just describe the bits this file reads off them.
export interface SetPlayer {
  slot: number;
  name: string | null;
  character: string | null;
  wins: number | null;
}

export interface SetGame {
  gameNum: number;
  winnerSlot: number | null;
  chars: Array<{ slot: number; character: string | null }>;
}

export interface Entrant {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  id: any;
  name: string;
}

export interface MappingRow {
  tag: string;
  entrant: string | null;
}

export function players(r: HubRecord): SetPlayer[] {
  return (r.set?.players ?? []) as SetPlayer[];
}

/** Games in play order, each side sorted by slot so the same slot always
 * lands on the same side across every game in the strip. */
export function games(r: HubRecord): SetGame[] {
  const gs = (r.set?.games ?? []) as SetGame[];
  return gs.map((g) => ({ ...g, chars: [...g.chars].sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0)) }));
}

export function entrants(r: HubRecord): Entrant[] {
  return (r.entrants ?? []) as Entrant[];
}

export function playersLabel(r: HubRecord): string {
  return players(r).map((p) => p.name ?? '?').join(' vs ');
}

export function score(r: HubRecord): string {
  return players(r).map((p) => p.wins ?? '?').join('–');
}

/** "first to N", or null when the station never learned the best-of --
 * `winsRequired` can be null (see matching.rs), and a guess would be worse
 * than nothing here. */
export function bestOf(r: HubRecord): string | null {
  const wr = r.set?.winsRequired;
  return wr || wr === 0 ? `first to ${wr}` : null;
}

function clockAt(epoch?: number | null): string {
  if (!epoch) return '·';
  const d = new Date(epoch * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** Minutes/hours since `startEpoch`, against a caller-supplied "now" (seconds)
 * so this stays deterministic under the screenshot harness's frozen clock and
 * doesn't need its own timer. */
export function elapsedSince(startEpoch: number | null | undefined, nowS: number): string {
  if (!startEpoch) return '·';
  const mins = Math.max(0, Math.floor((nowS - startEpoch) / 60));
  if (mins < 60) return `${mins}m`;
  return `${Math.floor(mins / 60)}h${String(mins % 60).padStart(2, '0')}m`;
}

/** The one thing the time column shows: elapsed for a live set, an absolute
 * clock for a finished one -- never the same field meaning two things. */
export function timeText(r: HubRecord, nowS: number): string {
  if (r.status === 'live') return elapsedSince(r.set?.startEpoch, nowS);
  return clockAt(r.set?.endEpoch ?? r.ingestedAt);
}

export function timeTitle(r: HubRecord, nowS: number): string {
  if (r.status === 'live') {
    return r.set?.startEpoch
      ? `Live ${elapsedSince(r.set.startEpoch, nowS)} (started ${clockAt(r.set.startEpoch)})`
      : 'Live, start time unknown';
  }
  return r.set?.endEpoch
    ? `Ended ${clockAt(r.set.endEpoch)}`
    : `Ingested ${clockAt(r.ingestedAt)} (end time unknown)`;
}

// ---- start.gg mapping -------------------------------------------------------

// Drives the visual weight of the mapping cell: 'high'/'low' mirror
// matching::match_winner's own confidence label, 'absent' covers every case
// where there's nothing (or nothing trustworthy) to show -- never let a weak
// guess read as a confirmed fact.
export function matchTier(r: HubRecord): 'high' | 'low' | 'none' | 'absent' {
  if (r.reportable === false) return 'absent';
  if (r.candidateWinnerEntrantId != null) return r.confidence === 'low' ? 'low' : 'high';
  if (entrants(r).length) return 'none';
  return 'absent';
}

export function matchTitle(r: HubRecord): string {
  if (r.reportable === false) {
    return `${r.notReportableReason || 'not a bracket set'}: not part of the bracket`;
  }
  if (r.candidateWinnerEntrantId != null) {
    return `Suggested start.gg mapping (confidence: ${r.confidence || 'none'}). Verify before reporting`;
  }
  if (entrants(r).length) return 'Bracket set found, but no name match for a winner guess';
  return 'Not matched to a start.gg set';
}

/** Per-player tag -> bracket-entrant name, both sides (not just the winner).
 *
 * Only derivable once a candidate exists: `match_winner` never returns one
 * until the set is complete and has a `winnerName` (matching.rs L184), so
 * this is null for every live set. Once it exists, with exactly two players
 * and two entrants the winner's slot anchors to the candidate entrant and the
 * other slot is forced onto the other entrant -- the same anchoring
 * `map_slots_to_entrants` does when finalizing a report (matching.rs L336+),
 * not a guess invented here. */
export function mappingRows(r: HubRecord): MappingRow[] | null {
  const ps = players(r);
  const es = entrants(r);
  if (ps.length !== 2) return null;
  if (es.length === 2 && r.candidateWinnerEntrantId != null) {
    const winnerSlot = r.set?.winnerSlot;
    const winnerEntrant = es.find((e) => String(e.id) === String(r.candidateWinnerEntrantId));
    const otherEntrant = es.find((e) => String(e.id) !== String(r.candidateWinnerEntrantId));
    return ps.map((p) => ({
      tag: p.name ?? '?',
      entrant: (winnerSlot != null && p.slot === winnerSlot ? winnerEntrant : otherEntrant)?.name ?? null,
    }));
  }
  if (es.length) {
    // A bracket set is bound, but no name matched confidently enough for a
    // winner guess -- both sides are honestly "unmatched", not silently
    // paired by order.
    return ps.map((p) => ({ tag: p.name ?? '?', entrant: null }));
  }
  return null;
}

/** Single-line fallback for when there's no per-player mapping to show at all
 * (no bracket entrants known, or the row isn't reportable). */
export function matchFallback(r: HubRecord): string {
  if (r.reportable === false) return '·';
  return 'not matched to start.gg';
}

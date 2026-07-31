// Coverage for the start.gg-over-local-inference preference logic added to
// operatorFormat.ts: preferredStartEpoch (elapsed-time source) and bestOf
// (best-of source). Both must prefer the start.gg-sourced field when present
// and fall back completely to the existing local inference otherwise -- see
// crates/station-core/src/hub.rs's `preferred_started_at` for the
// startedAt-over-startAt fallback this builds on (tested there in Rust,
// since that's where the two similarly-named start.gg fields are resolved
// down to the one value this file consumes).

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { bestOf, preferredStartEpoch } from './operatorFormat.ts';
import type { HubRecord } from '../types';

test('preferredStartEpoch prefers startggStartedAt over the local set.startEpoch guess', () => {
  const r: HubRecord = { startggStartedAt: 1000, set: { startEpoch: 2000 } };
  assert.equal(preferredStartEpoch(r), 1000);
});

test('preferredStartEpoch falls back to set.startEpoch when startggStartedAt is absent', () => {
  const r: HubRecord = { set: { startEpoch: 2000 } };
  assert.equal(preferredStartEpoch(r), 2000);
});

test('preferredStartEpoch falls back to set.startEpoch when startggStartedAt is null', () => {
  // An older cached binding from before this field existed, or a record the
  // hub never bound to a bracket set -- either way, null rather than absent.
  const r: HubRecord = { startggStartedAt: null, set: { startEpoch: 2000 } };
  assert.equal(preferredStartEpoch(r), 2000);
});

test('preferredStartEpoch is undefined when neither field is known', () => {
  assert.equal(preferredStartEpoch({}), undefined);
});

test('bestOf prefers startggTotalGames, converted to first-to-N wins, over winsRequired', () => {
  // best-of-7 (totalGames: 7) -> first to 4, even though the station's own
  // guess (winsRequired: 3, a best-of-5 guess) disagrees.
  const r: HubRecord = { startggTotalGames: 7, set: { winsRequired: 3 } };
  assert.equal(bestOf(r), 'first to 4');
});

test('bestOf falls back to winsRequired when startggTotalGames is absent', () => {
  const r: HubRecord = { set: { winsRequired: 3 } };
  assert.equal(bestOf(r), 'first to 3');
});

test('bestOf falls back to winsRequired when startggTotalGames is not a positive number', () => {
  const r: HubRecord = { startggTotalGames: null, set: { winsRequired: 2 } };
  assert.equal(bestOf(r), 'first to 2');
});

test('bestOf is null when neither startggTotalGames nor winsRequired is known', () => {
  assert.equal(bestOf({}), null);
  assert.equal(bestOf({ set: {} }), null);
});

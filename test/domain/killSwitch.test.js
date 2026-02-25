import test from 'node:test';
import assert from 'node:assert';
import {
  createKillSwitchState,
  checkKillSwitch,
  overrideKillSwitch,
  shouldResetKillSwitch,
  resetKillSwitch,
} from '../../src/domain/killSwitch.js';

// ── createKillSwitchState ───────────────────────────────────────────

test('killSwitch: createKillSwitchState returns correct initial state', () => {
  const state = createKillSwitchState();
  assert.strictEqual(state.active, false);
  assert.strictEqual(state.overrideActive, false);
  assert.strictEqual(state.overrideCount, 0);
  assert.deepStrictEqual(state.overrideLog, []);
  assert.strictEqual(state.lastResetDate, null);
});

// ── checkKillSwitch ─────────────────────────────────────────────────

test('killSwitch: triggers at exact threshold', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, -50, 50);
  assert.strictEqual(result.triggered, true);
  assert.ok(result.reason.includes('$-50.00'));
});

test('killSwitch: does NOT trigger below threshold', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, -49, 50);
  assert.strictEqual(result.triggered, false);
});

test('killSwitch: triggers when loss exceeds threshold', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, -75, 50);
  assert.strictEqual(result.triggered, true);
});

test('killSwitch: does NOT trigger on profit', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, 100, 50);
  assert.strictEqual(result.triggered, false);
});

test('killSwitch: disabled when maxDailyLossUsd is 0', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, -1000, 0);
  assert.strictEqual(result.triggered, false);
});

test('killSwitch: disabled when maxDailyLossUsd is null', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, -1000, null);
  assert.strictEqual(result.triggered, false);
});

test('killSwitch: override prevents trigger within 10% buffer', () => {
  const state = { ...createKillSwitchState(), overrideActive: true };
  // Limit is $50, so override allows up to $55 loss (10% buffer)
  const result = checkKillSwitch(state, -50, 50);
  assert.strictEqual(result.triggered, false);
  assert.strictEqual(result.overridden, true);
});

test('killSwitch: re-triggers after override when loss exceeds buffer', () => {
  const state = { ...createKillSwitchState(), overrideActive: true };
  // Override threshold = -50 * 1.1 = -55 (use -56 to avoid floating point edge)
  const result = checkKillSwitch(state, -56, 50);
  assert.strictEqual(result.triggered, true);
  assert.ok(result.reason.includes('override threshold'));
});

test('killSwitch: handles NaN todayRealizedPnl gracefully', () => {
  const state = createKillSwitchState();
  const result = checkKillSwitch(state, NaN, 50);
  assert.strictEqual(result.triggered, false);
});

// ── overrideKillSwitch ──────────────────────────────────────────────

test('killSwitch: overrideKillSwitch activates override', () => {
  const state = createKillSwitchState();
  const newState = overrideKillSwitch(state);

  assert.strictEqual(newState.overrideActive, true);
  assert.strictEqual(newState.overrideCount, 1);
  assert.strictEqual(newState.overrideLog.length, 1);
  assert.ok(newState.overrideLog[0].timestamp);
  assert.strictEqual(newState.overrideLog[0].count, 1);
});

test('killSwitch: multiple overrides increment count', () => {
  let state = createKillSwitchState();
  state = overrideKillSwitch(state);
  state = overrideKillSwitch(state);

  assert.strictEqual(state.overrideCount, 2);
  assert.strictEqual(state.overrideLog.length, 2);
});

test('killSwitch: override does not mutate original state', () => {
  const state = createKillSwitchState();
  const newState = overrideKillSwitch(state);

  assert.strictEqual(state.overrideActive, false);
  assert.strictEqual(state.overrideCount, 0);
  assert.strictEqual(newState.overrideActive, true);
});

// ── shouldResetKillSwitch ───────────────────────────────────────────

test('killSwitch: shouldResetKillSwitch returns true when lastResetDate is null', () => {
  const state = createKillSwitchState();
  assert.strictEqual(shouldResetKillSwitch(state), true);
});

test('killSwitch: shouldResetKillSwitch returns false when same day', () => {
  const now = new Date();
  const todayPT = new Intl.DateTimeFormat('en-CA', {
    timeZone: 'America/Los_Angeles',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(now);

  const state = { ...createKillSwitchState(), lastResetDate: todayPT };
  assert.strictEqual(shouldResetKillSwitch(state, now), false);
});

test('killSwitch: shouldResetKillSwitch returns true when different day', () => {
  const state = { ...createKillSwitchState(), lastResetDate: '2020-01-01' };
  assert.strictEqual(shouldResetKillSwitch(state, new Date()), true);
});

// ── resetKillSwitch ─────────────────────────────────────────────────

test('killSwitch: resetKillSwitch clears override but preserves log', () => {
  let state = createKillSwitchState();
  state = overrideKillSwitch(state);
  state = overrideKillSwitch(state);
  assert.strictEqual(state.overrideCount, 2);

  const reset = resetKillSwitch(state);

  assert.strictEqual(reset.active, false);
  assert.strictEqual(reset.overrideActive, false);
  assert.strictEqual(reset.overrideCount, 0);
  assert.strictEqual(reset.overrideLog.length, 2); // Preserved
  assert.ok(reset.lastResetDate); // Set to today
});

test('killSwitch: resetKillSwitch sets lastResetDate to today PT', () => {
  const state = createKillSwitchState();
  const now = new Date('2026-02-23T10:00:00Z');
  const reset = resetKillSwitch(state, now);

  // 10:00 UTC = 02:00 PT on Feb 23
  assert.ok(reset.lastResetDate);
  assert.strictEqual(typeof reset.lastResetDate, 'string');
  assert.ok(reset.lastResetDate.match(/^\d{4}-\d{2}-\d{2}$/));
});

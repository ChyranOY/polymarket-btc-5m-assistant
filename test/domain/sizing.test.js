import test from 'node:test';
import assert from 'node:assert/strict';

import { computeTradeSize, computeTradeSizeWithFees } from '../../src/domain/sizing.js';

// ─── Dynamic Sizing ────────────────────────────────────────────────

test('dynamic sizing: stakePct * balance', () => {
  const size = computeTradeSize(1000, { stakePct: 0.10 });
  assert.equal(size, 100); // 10% of 1000
});

test('dynamic sizing: respects maxTradeUsd', () => {
  const size = computeTradeSize(10000, { stakePct: 0.10, maxTradeUsd: 500 });
  assert.equal(size, 500); // 10% of 10000 = 1000, capped at 500
});

test('dynamic sizing: respects minTradeUsd', () => {
  const size = computeTradeSize(100, { stakePct: 0.01, minTradeUsd: 5 });
  assert.equal(size, 5); // 1% of 100 = 1, raised to min 5
});

test('dynamic sizing: capped at balance', () => {
  const size = computeTradeSize(50, { stakePct: 0.10, minTradeUsd: 100 });
  assert.equal(size, 50); // min 100 but only 50 available
});

// ─── Fixed Sizing ──────────────────────────────────────────────────

test('fixed sizing: uses contractSize when no stakePct', () => {
  const size = computeTradeSize(500, { contractSize: 100 });
  assert.equal(size, 100);
});

test('fixed sizing: default 100 when nothing configured', () => {
  const size = computeTradeSize(500, {});
  assert.equal(size, 100);
});

test('fixed sizing: capped at balance', () => {
  const size = computeTradeSize(50, { contractSize: 100 });
  assert.equal(size, 50);
});

// ─── Edge Cases ────────────────────────────────────────────────────

test('returns 0 for zero balance', () => {
  const size = computeTradeSize(0, { stakePct: 0.10 });
  assert.equal(size, 0);
});

test('returns 0 for negative balance', () => {
  const size = computeTradeSize(-100, { stakePct: 0.10 });
  assert.equal(size, 0);
});

test('returns 0 for NaN balance', () => {
  const size = computeTradeSize(NaN, { stakePct: 0.10 });
  assert.equal(size, 0);
});

test('rounds down to cents', () => {
  const size = computeTradeSize(333, { stakePct: 0.10 });
  // 333 * 0.10 = 33.3 → floor(33.3 * 100) / 100 = 33.30
  assert.equal(size, 33.30);
});

test('stakePct=0 falls back to fixed sizing', () => {
  const size = computeTradeSize(500, { stakePct: 0, contractSize: 75 });
  assert.equal(size, 75);
});

// ─── Fee-Aware Sizing ─────────────────────────────────────────────

test('fee-aware sizing: 200bps fee deduction', () => {
  // 1000 * 0.08 = 80, fee = 80 * (1 - 200/10000) = 80 * 0.98 = 78.40
  const size = computeTradeSizeWithFees(1000, { stakePct: 0.08 }, 200);
  assert.equal(size, 78.40);
});

test('fee-aware sizing: null feeRateBps falls back to regular sizing', () => {
  const raw = computeTradeSize(1000, { stakePct: 0.08 });
  const withFees = computeTradeSizeWithFees(1000, { stakePct: 0.08 }, null);
  assert.equal(withFees, raw);
});

test('fee-aware sizing: 0 feeRateBps returns same as computeTradeSize', () => {
  const raw = computeTradeSize(1000, { stakePct: 0.08 });
  const withFees = computeTradeSizeWithFees(1000, { stakePct: 0.08 }, 0);
  assert.equal(withFees, raw);
});

test('fee-aware sizing: below minTradeUsd after fee returns 0', () => {
  // 50 * 0.08 = 4, with 200bps fee = 4 * 0.98 = 3.92, below minTradeUsd=5
  const size = computeTradeSizeWithFees(50, { stakePct: 0.08, minTradeUsd: 5 }, 200);
  assert.equal(size, 0);
});

test('fee-aware sizing: fee rate clamped at 1000bps (10%)', () => {
  // 1000 * 0.10 = 100, with 5000bps (clamped to 1000bps) = 100 * 0.90 = 90
  const size = computeTradeSizeWithFees(1000, { stakePct: 0.10 }, 5000);
  assert.equal(size, 90);
});

test('fee-aware sizing: backward compat - existing computeTradeSize unchanged', () => {
  // Ensure original function still works
  const size = computeTradeSize(1000, { stakePct: 0.10 });
  assert.equal(size, 100);
});

test('fee-aware sizing: negative feeRateBps falls back to regular sizing', () => {
  const raw = computeTradeSize(1000, { stakePct: 0.08 });
  const withFees = computeTradeSizeWithFees(1000, { stakePct: 0.08 }, -100);
  assert.equal(withFees, raw);
});

test('fee-aware sizing: zero balance returns 0', () => {
  const size = computeTradeSizeWithFees(0, { stakePct: 0.10 }, 200);
  assert.equal(size, 0);
});

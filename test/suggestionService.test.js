import test from 'node:test';
import assert from 'node:assert/strict';

import {
  matchBlockerToThreshold,
  computeRelaxedValue,
  computeConfidence,
  generateSuggestions,
  BLOCKER_THRESHOLD_MAP,
} from '../src/services/suggestionService.js';

// ─── matchBlockerToThreshold ─────────────────────────────────────────

test('matchBlockerToThreshold matches known threshold blockers', () => {
  const result = matchBlockerToThreshold('Prob X < X');
  assert.ok(result);
  assert.equal(result.mapping.configKey, 'minProbMid');

  const edge = matchBlockerToThreshold('Edge X < X');
  assert.ok(edge);
  assert.equal(edge.mapping.configKey, 'edgeMid');

  const rsi = matchBlockerToThreshold('RSI in no-trade band (X in [N,N))');
  assert.ok(rsi);
  assert.equal(rsi.mapping.configKey, 'noTradeRsiMin');
});

test('matchBlockerToThreshold returns null for non-threshold blockers', () => {
  assert.equal(matchBlockerToThreshold('Trading disabled'), null);
  assert.equal(matchBlockerToThreshold('Trade already open'), null);
  assert.equal(matchBlockerToThreshold('Warmup: candles N/N'), null);
  assert.equal(matchBlockerToThreshold('Missing side'), null);
  assert.equal(matchBlockerToThreshold('Too late (<X.Xm to settlement)'), null);
});

test('matchBlockerToThreshold returns null for empty/null input', () => {
  assert.equal(matchBlockerToThreshold(null), null);
  assert.equal(matchBlockerToThreshold(''), null);
  assert.equal(matchBlockerToThreshold(undefined), null);
});

test('matchBlockerToThreshold matches with prefix for extended blocker text', () => {
  // Blocker keys may have extra text after the matched pattern
  const result = matchBlockerToThreshold('Low liquidity (<N)');
  assert.ok(result);
  assert.equal(result.mapping.configKey, 'minLiquidity');

  const spread = matchBlockerToThreshold('High spread');
  assert.ok(spread);
  assert.equal(spread.mapping.configKey, 'maxSpreadThreshold');
});

// ─── computeRelaxedValue ─────────────────────────────────────────────

test('computeRelaxedValue lowers value for lower direction', () => {
  const mapping = { direction: 'lower', step: 0.01, minBound: 0.50 };
  const result = computeRelaxedValue(0.53, mapping);
  assert.ok(result !== null);
  assert.ok(Math.abs(result - 0.52) < 0.0001);
});

test('computeRelaxedValue raises value for higher direction', () => {
  const mapping = { direction: 'higher', step: 0.001, maxBound: 0.015 };
  const result = computeRelaxedValue(0.005, mapping);
  assert.ok(result !== null);
  assert.ok(Math.abs(result - 0.006) < 0.0001);
});

test('computeRelaxedValue returns null when at minBound', () => {
  const mapping = { direction: 'lower', step: 0.01, minBound: 0.50 };
  const result = computeRelaxedValue(0.50, mapping);
  assert.equal(result, null);
});

test('computeRelaxedValue returns null when at maxBound', () => {
  const mapping = { direction: 'higher', step: 0.001, maxBound: 0.015 };
  const result = computeRelaxedValue(0.015, mapping);
  assert.equal(result, null);
});

test('computeRelaxedValue clamps to minBound', () => {
  const mapping = { direction: 'lower', step: 5, minBound: 15 };
  const result = computeRelaxedValue(17, mapping);
  assert.ok(result !== null);
  assert.equal(result, 15);
});

test('computeRelaxedValue returns null for non-numeric input', () => {
  const mapping = { direction: 'lower', step: 0.01, minBound: 0.50 };
  assert.equal(computeRelaxedValue(null, mapping), null);
  assert.equal(computeRelaxedValue(NaN, mapping), null);
  assert.equal(computeRelaxedValue(undefined, mapping), null);
});

// ─── computeConfidence ───────────────────────────────────────────────

test('computeConfidence returns green for 50+ trades', () => {
  assert.equal(computeConfidence(50), 'green');
  assert.equal(computeConfidence(100), 'green');
  assert.equal(computeConfidence(500), 'green');
});

test('computeConfidence returns yellow for 30-49 trades', () => {
  assert.equal(computeConfidence(30), 'yellow');
  assert.equal(computeConfidence(35), 'yellow');
  assert.equal(computeConfidence(49), 'yellow');
});

test('computeConfidence returns red for <30 trades', () => {
  assert.equal(computeConfidence(10), 'red');
  assert.equal(computeConfidence(29), 'red');
  assert.equal(computeConfidence(0), 'red');
});

test('computeConfidence returns red for invalid input', () => {
  assert.equal(computeConfidence(null), 'red');
  assert.equal(computeConfidence(NaN), 'red');
  assert.equal(computeConfidence(-5), 'red');
});

// ─── generateSuggestions ─────────────────────────────────────────────

test('generateSuggestions returns empty for empty trades', () => {
  const result = generateSuggestions([], { total: 100, topBlockers: [] }, {}, {});
  assert.deepEqual(result, []);
});

test('generateSuggestions returns empty for no threshold blockers', () => {
  const trades = createMockTrades(50);
  const blockerSummary = {
    total: 100,
    topBlockers: [
      { blocker: 'Trading disabled', count: 80, pct: 80 },
      { blocker: 'Trade already open', count: 60, pct: 60 },
    ],
  };
  const result = generateSuggestions(trades, blockerSummary, {}, {});
  assert.deepEqual(result, []);
});

test('generateSuggestions returns suggestions with correct shape', () => {
  const trades = createMockTrades(60);
  const blockerSummary = {
    total: 200,
    topBlockers: [
      { blocker: 'Prob X < X', count: 144, pct: 72 },
    ],
  };
  const currentConfig = { minProbMid: 0.53 };
  const baseConfig = { minProbMid: 0.53 };

  const result = generateSuggestions(trades, blockerSummary, currentConfig, baseConfig);

  // May or may not produce suggestions depending on trade PnL distribution
  // but if it does, verify shape
  for (const s of result) {
    assert.ok(s.configKey);
    assert.ok(s.label);
    assert.ok(isFinite(s.currentValue));
    assert.ok(isFinite(s.suggestedValue));
    assert.ok(s.blockerKey);
    assert.ok(isFinite(s.blockerFrequency));
    assert.ok(s.baseline);
    assert.ok(s.projected);
    assert.ok(isFinite(s.pfImprovement));
    assert.ok(['green', 'yellow', 'red'].includes(s.confidence));
  }
});

test('generateSuggestions returns max 3 suggestions', () => {
  const trades = createMockTrades(100);
  const blockerSummary = {
    total: 500,
    topBlockers: [
      { blocker: 'Prob X < X', count: 360, pct: 72 },
      { blocker: 'Edge X < X', count: 250, pct: 50 },
      { blocker: 'Low liquidity (<N)', count: 200, pct: 40 },
      { blocker: 'High spread', count: 150, pct: 30 },
      { blocker: 'Low impulse', count: 100, pct: 20 },
    ],
  };
  const currentConfig = { minProbMid: 0.53, edgeMid: 0.03, minLiquidity: 500, maxSpreadThreshold: 0.012, minSpotImpulse: 0.0003 };
  const baseConfig = { ...currentConfig };

  const result = generateSuggestions(trades, blockerSummary, currentConfig, baseConfig);
  assert.ok(result.length <= 3);
});

test('generateSuggestions sorts by PF improvement descending', () => {
  const trades = createMockTrades(80);
  const blockerSummary = {
    total: 300,
    topBlockers: [
      { blocker: 'Prob X < X', count: 210, pct: 70 },
      { blocker: 'Edge X < X', count: 180, pct: 60 },
    ],
  };
  const currentConfig = { minProbMid: 0.53, edgeMid: 0.03 };
  const baseConfig = { ...currentConfig };

  const result = generateSuggestions(trades, blockerSummary, currentConfig, baseConfig);

  // If multiple suggestions, verify descending PF improvement
  for (let i = 1; i < result.length; i++) {
    assert.ok(result[i - 1].pfImprovement >= result[i].pfImprovement,
      `Expected descending PF improvement: ${result[i-1].pfImprovement} >= ${result[i].pfImprovement}`);
  }
});

// ─── Mock data helper ────────────────────────────────────────────────

function createMockTrades(count) {
  const trades = [];
  for (let i = 0; i < count; i++) {
    trades.push({
      status: 'CLOSED',
      pnl: i % 3 === 0 ? -3 : 5,  // Win rate ~66%, PF positive
      side: i % 2 === 0 ? 'UP' : 'DOWN',
      entryPhase: 'MID',
      entryTime: `2026-02-${String(1 + (i % 28)).padStart(2, '0')}T12:00:00Z`,
      exitTime: `2026-02-${String(1 + (i % 28)).padStart(2, '0')}T12:05:00Z`,
      modelProbAtEntry: 0.52 + Math.random() * 0.06,
      edgeAtEntry: 0.02 + Math.random() * 0.04,
      rsiAtEntry: 30 + Math.random() * 40,
      spreadAtEntry: 0.005 + Math.random() * 0.01,
      liquidityAtEntry: 300 + Math.random() * 1000,
      spotImpulsePctAtEntry: 0.0002 + Math.random() * 0.0005,
    });
  }
  return trades;
}

function isFinite(v) {
  return typeof v === 'number' && Number.isFinite(v);
}

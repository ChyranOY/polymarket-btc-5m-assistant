/**
 * @file Suggestion service: maps blocker diagnostics to backtest-validated threshold adjustments.
 *
 * Connects three existing systems:
 * 1. Blocker diagnostics (TradingState._blockerCounts) - which thresholds block most entries
 * 2. Backtester (replayTrades) - validates whether relaxing a threshold improves results
 * 3. Config apply (POST /api/config) - lets user implement the suggestion
 *
 * Service layer -- orchestration only, no direct I/O. Pure functions except for the
 * replayTrades dependency.
 */

import { replayTrades } from '../domain/backtester.js';

// ─── Blocker-to-Threshold Mapping ────────────────────────────────────

/**
 * Maps normalized blocker patterns (from TradingState._normalizeBlockerKey)
 * to configurable thresholds with relaxation parameters.
 *
 * Blocker keys are normalized: decimals replaced with X, integers with N.
 * We use startsWith matching since blocker text may have trailing content.
 */
export const BLOCKER_THRESHOLD_MAP = {
  'Prob X < X':            { configKey: 'minProbMid',         direction: 'lower',  step: 0.01,   minBound: 0.50,  maxBound: null, label: 'Min Probability (Mid)' },
  'Edge X < X':            { configKey: 'edgeMid',            direction: 'lower',  step: 0.005,  minBound: 0.005, maxBound: null, label: 'Edge Threshold (Mid)' },
  'RSI in no-trade band':  { configKey: 'noTradeRsiMin',      direction: 'lower',  step: 5,      minBound: 15,    maxBound: null, label: 'RSI No-Trade Min' },
  'Entry price too high':  { configKey: 'maxEntryPolyPrice',  direction: 'higher', step: 0.001,  minBound: null,  maxBound: 0.015, label: 'Max Entry Poly Price' },
  'Low liquidity':         { configKey: 'minLiquidity',       direction: 'lower',  step: 100,    minBound: 0,     maxBound: null, label: 'Min Liquidity' },
  'High spread':           { configKey: 'maxSpreadThreshold', direction: 'higher', step: 0.002,  minBound: null,  maxBound: 0.030, label: 'Max Spread Threshold' },
  'Low impulse':           { configKey: 'minSpotImpulse',     direction: 'lower',  step: 0.0001, minBound: 0,     maxBound: null, label: 'Min Spot Impulse' },
  'Choppy':                { configKey: 'minRangePct20',      direction: 'lower',  step: 0.0002, minBound: 0,     maxBound: null, label: 'Min Range Pct (20)' },
  'Low conviction':        { configKey: 'minModelMaxProb',    direction: 'lower',  step: 0.01,   minBound: 0.50,  maxBound: null, label: 'Min Model Max Prob' },
};

// ─── Helpers ─────────────────────────────────────────────────────────

function isNum(x) {
  return typeof x === 'number' && Number.isFinite(x);
}

/**
 * Match a normalized blocker key to a threshold mapping entry.
 * Uses startsWith matching since blocker keys may have trailing data.
 *
 * @param {string} blockerKey - Normalized blocker key from TradingState
 * @returns {{ pattern: string, mapping: Object }|null}
 */
export function matchBlockerToThreshold(blockerKey) {
  if (!blockerKey || typeof blockerKey !== 'string') return null;

  for (const [pattern, mapping] of Object.entries(BLOCKER_THRESHOLD_MAP)) {
    if (blockerKey.startsWith(pattern)) {
      return { pattern, mapping };
    }
  }
  return null;
}

/**
 * Compute a relaxed value for a threshold.
 *
 * @param {number} currentValue - Current threshold value
 * @param {Object} mapping - Entry from BLOCKER_THRESHOLD_MAP
 * @returns {number|null} Relaxed value, or null if already at boundary
 */
export function computeRelaxedValue(currentValue, mapping) {
  if (!isNum(currentValue) || !mapping) return null;

  let relaxed;
  if (mapping.direction === 'lower') {
    relaxed = currentValue - mapping.step;
    if (isNum(mapping.minBound) && relaxed < mapping.minBound) relaxed = mapping.minBound;
    // Already at or below bound
    if (relaxed >= currentValue) return null;
  } else {
    relaxed = currentValue + mapping.step;
    if (isNum(mapping.maxBound) && relaxed > mapping.maxBound) relaxed = mapping.maxBound;
    // Already at or above bound
    if (relaxed <= currentValue) return null;
  }

  // Round to avoid floating point noise (use step precision)
  const stepStr = String(mapping.step);
  const decimalPlaces = stepStr.includes('.') ? stepStr.split('.')[1].length : 0;
  const multiplier = Math.pow(10, decimalPlaces);
  relaxed = Math.round(relaxed * multiplier) / multiplier;

  return relaxed;
}

/**
 * Compute confidence level based on trade count.
 *
 * @param {number} tradeCount
 * @returns {'green'|'yellow'|'red'}
 */
export function computeConfidence(tradeCount) {
  if (!isNum(tradeCount) || tradeCount < 0) return 'red';
  if (tradeCount >= 50) return 'green';
  if (tradeCount >= 30) return 'yellow';
  return 'red';
}

// ─── Main Suggestion Generator ──────────────────────────────────────

/**
 * Generate backtest-validated threshold adjustment suggestions.
 *
 * Maps high-frequency blockers to configurable thresholds, runs backtests
 * with relaxed values, and surfaces up to 3 suggestions that improve PF.
 *
 * @param {Array} trades - Historical trades from ledger
 * @param {{ total: number, topBlockers: Array<{ blocker: string, count: number, pct: number }> }} blockerSummary
 * @param {Object} currentConfig - Current engine config (threshold keys)
 * @param {Object} baseConfig - Base config for backtester
 * @returns {Array} Sorted array of suggestion objects (max 3)
 */
export function generateSuggestions(trades, blockerSummary, currentConfig, baseConfig) {
  if (!Array.isArray(trades) || trades.length === 0) return [];
  if (!blockerSummary || !Array.isArray(blockerSummary.topBlockers)) return [];
  if (!currentConfig || typeof currentConfig !== 'object') return [];
  if (!baseConfig || typeof baseConfig !== 'object') return [];

  // Run baseline backtest (current config)
  const baseline = replayTrades(trades, {}, baseConfig);
  if (!baseline || baseline.tradeCount === 0) return [];

  const suggestions = [];

  // Process blockers by frequency (most frequent first)
  const sortedBlockers = [...blockerSummary.topBlockers].sort((a, b) => b.pct - a.pct);

  for (const blockerEntry of sortedBlockers) {
    const match = matchBlockerToThreshold(blockerEntry.blocker);
    if (!match) continue;

    const { mapping } = match;
    const configKey = mapping.configKey;

    // Get current value from config
    const currentValue = currentConfig[configKey] ?? baseConfig[configKey];
    if (!isNum(currentValue)) continue;

    // Compute relaxed value
    const relaxedValue = computeRelaxedValue(currentValue, mapping);
    if (relaxedValue === null) continue;

    // Run backtest with relaxed value
    const projected = replayTrades(trades, { [configKey]: relaxedValue }, baseConfig);
    if (!projected) continue;

    // Only surface if PF improves
    const baselinePF = baseline.profitFactor ?? 0;
    const projectedPF = projected.profitFactor ?? 0;

    if (projectedPF <= baselinePF) continue;

    suggestions.push({
      configKey,
      label: mapping.label,
      currentValue,
      suggestedValue: relaxedValue,
      blockerKey: blockerEntry.blocker,
      blockerFrequency: blockerEntry.pct,
      baseline: {
        winRate: baseline.winRate,
        profitFactor: baseline.profitFactor,
        tradeCount: baseline.tradeCount,
      },
      projected: {
        winRate: projected.winRate,
        profitFactor: projected.profitFactor,
        tradeCount: projected.tradeCount,
      },
      pfImprovement: projectedPF - baselinePF,
      confidence: computeConfidence(projected.tradeCount),
    });
  }

  // Sort by PF improvement descending
  suggestions.sort((a, b) => b.pfImprovement - a.pfImprovement);

  // Return top 3
  return suggestions.slice(0, 3);
}

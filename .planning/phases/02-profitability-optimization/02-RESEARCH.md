# Phase 2: Profitability Optimization - Research

**Researched:** 2026-02-23
**Status:** Complete

## Objective

Research how to implement segmented performance views (PROF-03) and blocker-based suggestion engine (PROF-04) using existing Phase 1 infrastructure.

## Existing Infrastructure (Phase 1 Foundation)

### Available Data Sources

1. **Trade Enrichment Fields (per trade in ledger):**
   - `entryPhase`: EARLY/MID/LATE classification (from rec object)
   - `rsiAtEntry`: RSI at entry time (used for market regime classification)
   - `entryTime`: UTC timestamp (used for session classification)
   - `modelProbAtEntry`, `edgeAtEntry`, `spreadAtEntry`, `liquidityAtEntry`, `spotImpulsePctAtEntry`
   - `exitReason`, `pnl`, `side`, `sideInferred`, `recActionAtEntry`

2. **Blocker Diagnostics (TradingState):**
   - `_blockerCounts`: Map<string, number> — normalized blocker key to count
   - `_totalEntryChecks`: number — total ticks evaluated
   - `getBlockerSummary(topN)`: returns `{ total, topBlockers: [{ blocker, count, pct }] }`
   - Blocker keys normalized by `_normalizeBlockerKey()`: replaces numbers with N/X

3. **Analytics Service:**
   - `groupSummary(trades, keyFn)`: Generic grouping with win/loss/winRate/avgPnl per bucket
   - `sessionKeyFromTrade(trade)`: Session classification (Asia/London/NY/Off-hours)
   - `computeAnalytics(trades)`: Returns 20+ groupings including byEntryPhase, bySession, byEntryRsiBucket

4. **Backtester:**
   - `replayTrades(trades, overrideConfig, baseConfig)`: Pure function, returns metrics for modified thresholds
   - `evaluateHistoricalEntry(trade, config)`: Tests 6 configurable thresholds against historical data

5. **Config System:**
   - `POST /api/config`: Apply parameter changes to running engine
   - `POST /api/config/revert`: Revert to previous config
   - `GET /api/config/current`: Get current threshold values

6. **Server Endpoints:**
   - `GET /api/diagnostics`: Returns blocker summary, effective thresholds, weekend state
   - `GET /api/analytics`: Returns all analytics groupings
   - `POST /api/backtest`: Run backtest with parameter overrides

## PROF-03: Segmented Performance Views

### Design

The segmented views need three dimensions not currently surfaced in the analytics tab UI:
1. **Entry Phase** — Already computed as `byEntryPhase` in `computeAnalytics()`
2. **Time of Day (Session)** — Already computed as `bySession` in `computeAnalytics()`
3. **Market Regime (RSI-based)** — Already computed as `byEntryRsiBucket` in `computeAnalytics()`

**Key insight:** The data is already computed server-side. PROF-03 is primarily a UI rendering task — adding a "Segmented Performance" section to the Analytics tab that displays these three groupings with win rate and profit factor prominently.

### Market Regime Classification

Per CONTEXT.md decision: RSI-based regime using `rsiAtEntry`:
- Oversold: RSI < 30
- Ranging: RSI 30-70
- Overbought: RSI > 70

The existing `byEntryRsiBucket` uses finer-grained buckets (<30, 30-45, 45-55, 55-70, 70+). For PROF-03, we need a coarser "regime" grouping. Options:
1. **Add a new grouping function** `regimeKeyFromTrade(trade)` that returns Oversold/Ranging/Overbought
2. **Client-side aggregation** of existing RSI buckets into regimes

**Recommendation:** Add `byMarketRegime` server-side in `computeAnalytics()` using a new key function. This keeps the UI simple and follows the existing pattern.

### Profit Factor Computation

The existing `groupSummary()` returns `{ key, count, pnl, wins, losses, winRate, avgPnl }` but does NOT include `profitFactor`. Need to extend it or compute PF client-side.

**Recommendation:** Extend `groupSummary()` to include `profitFactor` (winPnl / |lossPnl|) per bucket. This is a small change and keeps all metrics server-side.

### UI Layout

Add a "Segmented Performance" card in the Analytics tab with sub-tabs:
- **By Entry Phase** (EARLY/MID/LATE)
- **By Session** (Asia/London/NY/Off-hours)
- **By Market Regime** (Oversold/Ranging/Overbought)

Each shows a table: Segment | Trades | Win Rate | PF | PnL | Avg PnL

## PROF-04: Blocker-Based Suggestion Engine

### Architecture

The suggestion engine is the core of Phase 2. It connects three existing systems:
1. **Blocker diagnostics** → identifies which thresholds block most entries
2. **Backtester** → validates whether relaxing a threshold actually improves results
3. **Config apply** → lets user implement the suggestion

### Blocker-to-Threshold Mapping

Each normalized blocker key maps to a specific configurable threshold:

| Blocker Pattern | Config Key | Direction |
|-----------------|-----------|-----------|
| `Prob X < X` | `minProbMid` | Lower = more entries |
| `Edge X < X` | `edgeMid` | Lower = more entries |
| `RSI in no-trade band` | `noTradeRsiMin`, `noTradeRsiMax` | Narrower band = more entries |
| `Entry price too high` | `maxEntryPolyPrice` | Higher = more entries |
| `Low liquidity` | `minLiquidity` | Lower = more entries |
| `High spread` | `maxSpreadThreshold` | Higher = more entries |
| `Low impulse` | `minSpotImpulse` | Lower = more entries |
| `Choppy` | `minRangePct20` | Lower = more entries |
| `Low conviction` | `minModelMaxProb` | Lower = more entries |

Not all blockers are threshold-based. Non-threshold blockers (Trading disabled, Trade already open, Warmup, Market closed, etc.) cannot be adjusted and should be excluded from suggestions.

### Suggestion Generation Algorithm

```
1. Fetch blocker summary from /api/diagnostics (or compute from state)
2. Filter to threshold-based blockers (using mapping table)
3. For top N most frequent threshold blockers:
   a. Determine current threshold value
   b. Compute relaxed value (e.g., lower minProbMid by 0.01)
   c. Run backtest with relaxed value
   d. Compare: original WR/PF vs relaxed WR/PF
   e. Only surface if relaxed version improves profit factor
4. Rank by projected PF improvement
5. Return top 3 suggestions
```

### Relaxation Strategy

For each threshold, define a "relaxation step" that represents a meaningful but conservative change:

| Config Key | Relaxation Step | Min Bound |
|-----------|----------------|-----------|
| `minProbMid` | -0.01 | 0.50 |
| `edgeMid` | -0.005 | 0.005 |
| `noTradeRsiMin` | -5 | 15 |
| `noTradeRsiMax` | +5 | 60 |
| `maxEntryPolyPrice` | +0.001 | 0.010 |
| `minLiquidity` | -100 | 0 |
| `maxSpreadThreshold` | +0.002 | 0.020 |
| `minSpotImpulse` | -0.0001 | 0 |

### Service Layer Design

Create `src/services/suggestionService.js`:
- `generateSuggestions(trades, blockerSummary, currentConfig)` — Pure orchestration
  - Maps blockers to thresholds
  - For each mappable blocker, runs a backtest with the relaxed value
  - Computes improvement delta
  - Returns sorted suggestions

Each suggestion object:
```javascript
{
  configKey: 'minProbMid',
  label: 'Min Probability (Mid Phase)',
  currentValue: 0.53,
  suggestedValue: 0.52,
  blockerKey: 'Prob X < X',
  blockerFrequency: 72,  // percentage of ticks blocked
  backtestResult: {
    currentWinRate: 0.42,
    projectedWinRate: 0.45,
    currentPF: 1.2,
    projectedPF: 1.4,
    currentTradeCount: 150,
    projectedTradeCount: 220,
  },
  confidence: 'green',  // green (50+ trades), yellow (30-49), red (<30)
  reasoning: 'Blocked 72% of entries. Backtest shows +3% WR, +0.2 PF with 47% more trades.',
}
```

### Confidence Levels

Per CONTEXT.md decision:
- Green: 50+ trades backing the projected metrics
- Yellow: 30-49 trades
- Red: <30 trades (speculative)

These thresholds align with the grid search optimizer's minimum of 30 trades.

### Auto-Refresh Trigger

Per CONTEXT.md: refresh suggestions every N closed trades. Given typical trade frequency of 20-50 trades/day in paper mode:
- **N = 20 trades** — recomputes after each meaningful batch
- Track `lastSuggestionTradeCount` and compare against current closed trade count
- Show "N new trades since last analysis" badge

### Post-Apply Tracking

After a suggestion is applied:
- Record: `{ configKey, suggestedValue, projectedWR, projectedPF, appliedAt, tradeCountAtApply }`
- On subsequent analytics fetches, compute actual performance on trades after apply
- Display "Since Applied" card: projected vs actual WR/PF with trade count
- If actual PF < projected PF * 0.7 (30% underperformance), show "Underperforming" badge

### API Design

New endpoints:
- `GET /api/suggestions` — Generate and return current suggestions
- `POST /api/suggestions/apply` — Apply a specific suggestion (delegates to existing /api/config)
- `GET /api/suggestions/tracking` — Get post-apply tracking data

### UI Design

New section in Analytics tab (below period tables, above drawdown chart):

**"Suggested Adjustments" section:**
- Each suggestion is a card with:
  - Parameter name and current → suggested value
  - Blocker frequency badge (e.g., "Blocked 72% of entries")
  - Before/after metrics: WR, PF, Trade frequency
  - Confidence indicator (green/yellow/red dot)
  - "Apply" button
- After applying, the card transforms to "Since Applied" view:
  - Projected vs Actual WR/PF
  - Trade count since apply
  - Performance badge (On Track / Underperforming)

## Implementation Risks

1. **Insufficient blocker data**: If the bot hasn't run long enough, blocker counts will be low. Mitigate: show "Insufficient data" message when totalEntryChecks < 100.

2. **Backtest sample size**: Relaxing a threshold changes the trade set. If the projected trade count is <30, confidence is red. The UI should clearly communicate this uncertainty.

3. **Stale suggestions**: Blocker frequencies change over time (market conditions shift). The auto-refresh mechanism (every 20 trades) prevents stale suggestions from persisting.

4. **Multiple threshold interactions**: Changing one threshold may affect the optimal value of another. This phase surfaces one-at-a-time suggestions; multi-parameter optimization is already handled by the Phase 1 grid search.

## File Change Summary

| File | Change Type | Purpose |
|------|------------|---------|
| src/services/analyticsService.js | Modify | Add byMarketRegime, extend groupSummary with profitFactor |
| src/services/suggestionService.js | New | Suggestion generation, blocker-to-threshold mapping |
| src/ui/server.js | Modify | Add /api/suggestions, /api/suggestions/apply, /api/suggestions/tracking |
| src/ui/analytics.js | Modify | Add segmented performance tables, suggestion cards |
| src/ui/index.html | Modify | Add HTML structure for segmentation and suggestions |
| src/ui/style.css | Modify | Style suggestion cards, confidence badges, tracking cards |
| test/analyticsService.test.js | Modify | Test byMarketRegime, profitFactor in groupSummary |
| test/suggestionService.test.js | New | Test suggestion generation, blocker mapping, confidence |

---

## RESEARCH COMPLETE

Research covers both PROF-03 and PROF-04 requirements with specific implementation patterns, API designs, and UI layouts.

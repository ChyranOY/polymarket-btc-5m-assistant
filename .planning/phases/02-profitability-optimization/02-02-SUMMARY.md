# Plan 02-02 Summary: Suggestion Engine (PROF-04)

**Status:** Complete
**Requirement:** PROF-04 — Entry filter adjustments suggested based on blocker diagnostics frequency data
**Wave:** 2 (depends on 02-01)

## What Was Built

### Task 1: Suggestion service with blocker-to-threshold mapping

**Files created:**
- `src/services/suggestionService.js`
- `test/suggestionService.test.js`

**Changes:**
1. Created `BLOCKER_THRESHOLD_MAP` — maps 9 normalized blocker patterns to config keys with relaxation parameters:
   - Prob X < X -> minProbMid (lower by 0.01, min 0.50)
   - Edge X < X -> edgeMid (lower by 0.005, min 0.005)
   - RSI in no-trade band -> noTradeRsiMin (lower by 5, min 15)
   - Entry price too high -> maxEntryPolyPrice (higher by 0.001, max 0.015)
   - Low liquidity -> minLiquidity (lower by 100, min 0)
   - High spread -> maxSpreadThreshold (higher by 0.002, max 0.030)
   - Low impulse -> minSpotImpulse (lower by 0.0001, min 0)
   - Choppy -> minRangePct20 (lower by 0.0002, min 0)
   - Low conviction -> minModelMaxProb (lower by 0.01, min 0.50)

2. `matchBlockerToThreshold(blockerKey)` — prefix matching against BLOCKER_THRESHOLD_MAP
3. `computeRelaxedValue(currentValue, mapping)` — applies step in direction, clamps to bounds, rounds to step precision
4. `computeConfidence(tradeCount)` — traffic light: green (50+), yellow (30-49), red (<30)
5. `generateSuggestions(trades, blockerSummary, currentConfig, baseConfig)`:
   - Runs baseline backtest via replayTrades
   - For each high-frequency threshold blocker: compute relaxed value, run projected backtest
   - Only surfaces suggestions where projected PF > baseline PF
   - Returns top 3 sorted by PF improvement descending
   - Each suggestion includes: configKey, label, currentValue, suggestedValue, blockerKey, blockerFrequency, baseline/projected metrics, pfImprovement, confidence

6. Tests: 16 test cases covering matchBlockerToThreshold, computeRelaxedValue, computeConfidence, generateSuggestions (empty input, non-threshold blockers, correct shape, max 3, sorted by PF)

### Task 2: Suggestion API endpoints

**Files modified:**
- `src/ui/server.js`

**Changes:**
1. `GET /api/suggestions` — generates suggestions from trades + blocker data:
   - Guards: engine required, minimum 100 entry checks
   - Loads trades, builds currentConfig/baseConfig, calls generateSuggestions()
   - Tracks lastSuggestionTradeCount for refresh badge
   - Returns suggestions array + tradesSinceLastAnalysis

2. `POST /api/suggestions/apply` — applies a suggestion:
   - Validates configKey against allowed set
   - Maps configKey to engine config key (maxSpreadThreshold->maxSpread, minSpotImpulse->minBtcImpulsePct1m)
   - Records tracking data in globalThis.__appliedSuggestions
   - Returns success with applied value

3. `GET /api/suggestions/tracking` — post-apply comparison:
   - For each applied suggestion, counts trades since apply
   - Computes actual WR and PF from post-apply trades
   - Flags as 'underperforming' if actual PF < projected PF * 0.7

### Task 3: Suggestion cards UI in Analytics tab

**Files modified:**
- `src/ui/index.html`
- `src/ui/analytics.js`
- `src/ui/style.css`

**Changes:**
1. HTML: Added "Suggested Adjustments" card with refresh badge and "Post-Apply Tracking" card (hidden by default)
2. `fetchAndRenderSuggestions()` — fetches /api/suggestions, handles insufficient data, renders cards
3. `renderSuggestionCards(suggestions)` — renders per-suggestion card:
   - Confidence dot (green/yellow/red)
   - Parameter name with current -> suggested values
   - Blocker frequency badge
   - Before/after metrics table (WR, PF, Trades with change column)
   - Apply button with POST handler
4. `fetchAndRenderTracking()` — renders projected vs actual after apply:
   - Shows each applied suggestion with trade count since apply
   - Status badge: "On Track" (green) or "Underperforming" (red)
5. CSS: `.suggestion-card`, `.confidence-dot`, `.confidence-green/yellow/red`, `.blocker-freq-badge`, `.suggestion-metrics-table`, `.apply-suggestion-btn`, `.tracking-record`, `.status-badge-ontrack`, `.status-badge-underperforming`, `.suggestion-change-positive/negative`
6. Auto-refresh: fetches suggestions when Analytics tab is selected

## Verification

### Automated (run manually)
```bash
cd /Users/elijahprince/Dev/polymarket-btc-5m-assistant
node --test test/suggestionService.test.js
```

### Manual
- GET /api/suggestions returns suggestion array (or insufficient data message)
- Each suggestion has correct shape (configKey, projected, confidence, pfImprovement)
- Only PF-improving suggestions surfaced (verified by backtest)
- POST /api/suggestions/apply applies config change
- GET /api/suggestions/tracking shows projected vs actual metrics
- Analytics tab shows suggestion cards with confidence dots
- Apply button triggers config change and refreshes
- Tracking card appears after applying a suggestion

## Requirements Satisfied

- **PROF-04**: Entry filter adjustments suggested based on blocker diagnostics frequency data
  - Maps blocker frequency data to configurable thresholds (9 mappings)
  - Validates each adjustment via backtesting before surfacing
  - Shows expected impact on WR, PF, and trade frequency
  - One-click apply through existing config system
  - Post-apply tracking compares projected vs actual performance
  - Traffic light confidence based on trade count

## Git Commands (manual execution)

```bash
git add src/services/suggestionService.js test/suggestionService.test.js src/ui/server.js src/ui/analytics.js src/ui/index.html src/ui/style.css
git commit -m "feat: suggestion engine with backtest-validated threshold adjustments (PROF-04)

- Create suggestionService with BLOCKER_THRESHOLD_MAP (9 blocker-to-config mappings)
- Add generateSuggestions with backtest validation via replayTrades
- Add GET/POST /api/suggestions endpoints with post-apply tracking
- Add suggestion cards UI with confidence dots, metrics tables, Apply buttons
- Traffic light confidence: green (50+), yellow (30-49), red (<30 trades)
- Post-apply tracking shows projected vs actual WR/PF

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

# Phase 2: Profitability Optimization - Context

**Gathered:** 2026-02-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Use Phase 1 analytics data to tune entry/exit thresholds and improve trade selection quality. This phase adds segmented win rate and profit factor views (by entry phase, trading session, and market regime) to the Analytics tab, and builds an intelligent suggestion engine that analyzes blocker diagnostics data to recommend specific threshold adjustments — validated by backtesting before surfacing.

Requirements covered: PROF-03, PROF-04.

What this phase does NOT include: new indicator development, ML-based signals, multi-strategy support, live trading changes, infrastructure changes.

</domain>

<decisions>
## Implementation Decisions

### Segmentation Dimensions
- Entry phase grouping uses existing EARLY/MID/LATE from the `entryPhase` field already captured per trade — no new classification needed
- Time-of-day segmentation uses session-based grouping only (Asia, London, New York, Off-hours) — reuse the session classification from Phase 1's `analyticsService.js`
- Market condition classified by RSI-based regime: Oversold (<30), Ranging (30-70), Overbought (>70) — uses the `rsiAtEntry` field already stored per trade
- Only closed trades included in segmentation views — no unrealized PnL distortion

### Suggestion Presentation
- Suggestions show exact values with reasoning: "Change RSI min: 30 → 25 (blocked 72% of entries, backtest shows +8% WR with relaxed band)"
- Traffic light confidence indicator: Green (50+ trades backing suggestion), Yellow (30-49), Red (<30 — treat as speculative). Matches the 30-trade minimum from the grid search optimizer
- Suggestions appear as a new "Suggested Adjustments" section in the existing Analytics tab — below period performance tables
- Each suggestion includes inline before/after comparison: current WR/PF vs projected WR/PF, plus trade frequency impact

### Blocker-to-Suggestion Logic
- Backtest-validated only: system only suggests relaxing a threshold if backtesting confirms the relaxed value improves profit factor or win rate. High blocker frequency alone is not sufficient — the blocker might be correctly filtering bad trades
- Surface top 3 most impactful suggestions ranked by projected profit factor improvement
- Each suggestion shows trade frequency impact alongside quality metrics (e.g., "Trades/day: 12 → 18 (+50%), Win Rate: 42% → 45%, PF: 1.2 → 1.4")
- Auto-refresh suggestions every N closed trades (Claude picks appropriate N). Always shows count of new trades since last analysis

### Apply & Validate Flow
- Apply immediately using existing one-click config apply (POST /api/config) — no redundant backtest since suggestion already shows projected results
- One suggestion applied at a time — individual Apply buttons per suggestion card. No batch apply
- Post-apply tracking: track trades after config change, compare actual win rate/PF to projected. Display as "Since Applied" card showing projected vs actual with trade count
- No auto-revert: if actual performance diverges significantly from projected, show "Underperforming" warning badge. User decides whether to revert manually

### Claude's Discretion
- Exact auto-refresh trigger threshold (every N trades — Claude picks N based on typical trade frequency)
- "Since Applied" card visual design and placement within Analytics tab
- Warning badge visual design and divergence threshold for triggering it
- RSI regime boundary values (can adjust from default 30/70 if analysis suggests better breakpoints)
- Segmentation table layout and styling choices
- How to handle segments with very few trades (minimum display threshold)

</decisions>

<specifics>
## Specific Ideas

- The existing blocker frequency tracking in TradingState (added earlier this session) provides the raw data: `_blockerCounts` Map with normalized blocker keys and `_totalEntryChecks` counter
- The backtest harness from Phase 1 (`replayTrades()` in `backtester.js`) can validate each suggestion by replaying with the modified threshold
- The config apply/revert system from Phase 1 (`POST /api/config`, `POST /api/config/revert`) handles the apply flow
- Period grouping functions from `analyticsService.js` (byDay, byWeek, bySession) already exist and can be extended for segmentation

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 02-profitability-optimization*
*Context gathered: 2026-02-23*

# Plan 02-01 Summary: Segmented Performance (PROF-03)

**Status:** Complete
**Requirement:** PROF-03 — Win rate and profit factor segmented by entry phase, time of day, and market conditions
**Wave:** 1

## What Was Built

### Task 1: byMarketRegime grouping + profitFactor in groupSummary

**Files modified:**
- `src/services/analyticsService.js`
- `test/analyticsService.test.js`

**Changes:**
1. Extended `groupSummary()` to track `winPnl` and `lossPnl` per bucket and compute `profitFactor = winPnl / |lossPnl|` (null if no losses)
2. Added `regimeKeyFromTrade(trade)` — classifies trades by RSI at entry:
   - RSI < 30: `'Oversold'`
   - RSI 30-70: `'Ranging'`
   - RSI >= 70: `'Overbought'`
   - Missing/invalid: `'unknown'`
3. Added `byMarketRegime: groupSummary(closed, regimeKeyFromTrade)` to `computeAnalytics()` return
4. Added tests:
   - `regimeKeyFromTrade` for each regime bucket + unknown
   - `groupSummary` profitFactor computation (wins+losses, only wins, only losses)
   - `computeAnalytics` includes `byMarketRegime` key

### Task 2: Segmented performance UI in Analytics tab

**Files modified:**
- `src/ui/index.html`
- `src/ui/analytics.js`
- `src/ui/style.css`

**Changes:**
1. Added "Segmented Performance" card to Analytics tab with 3 sub-tabs:
   - By Entry Phase (EARLY/MID/LATE)
   - By Session (Asia/London/NY/Off-hours)
   - By Market Regime (Oversold/Ranging/Overbought)
2. Added `renderSegmentedTable(data)` in analytics.js:
   - Maps activeSegment to correct analytics data key
   - Renders table: Segment | Trades | Win Rate | PF | PnL | Avg PnL
   - Low-confidence indicator for segments with < 5 trades (dimmed opacity)
   - Profit factor color coding: green >= 1.0, red < 1.0
   - Sorted by PnL descending
3. Added CSS: `.seg-tab-btn`, `.segmented-table`, `.low-confidence`, `.pf-good`, `.pf-bad`

## Verification

### Automated (run manually)
```bash
cd /Users/elijahprince/Dev/polymarket-btc-5m-assistant
node --test test/analyticsService.test.js
```

### Manual
- GET /api/analytics should include `byMarketRegime` in response
- All groupSummary results should include `profitFactor` field
- Analytics tab shows "Segmented Performance" card with 3 clickable sub-tabs
- Each sub-tab renders table with WR, PF, PnL columns
- Segments with < 5 trades appear dimmed

## Requirements Satisfied

- **PROF-03**: Win rate and profit factor segmented by entry phase, time of day, and market conditions
  - Entry phase: byEntryPhase (EARLY/MID/LATE) -- already existed, now has PF + UI
  - Time of day: bySession (Asia/London/NY/Off-hours) -- already existed, now has PF + UI
  - Market conditions: byMarketRegime (Oversold/Ranging/Overbought) -- new grouping + UI

## Git Commands (manual execution)

```bash
git add src/services/analyticsService.js test/analyticsService.test.js src/ui/index.html src/ui/analytics.js src/ui/style.css
git commit -m "feat: segmented performance views — entry phase, session, market regime (PROF-03)

- Add profitFactor to groupSummary (winPnl / |lossPnl|)
- Add regimeKeyFromTrade classifying RSI into Oversold/Ranging/Overbought
- Add byMarketRegime to computeAnalytics
- Add segmented performance UI with 3 sub-tabs in Analytics tab
- Low-confidence indicator for segments with < 5 trades

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

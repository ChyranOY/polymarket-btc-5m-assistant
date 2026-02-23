# Dashboard Redesign — Design Document

**Date:** 2026-02-22
**Status:** Approved

## Goal

Redesign the trading dashboard from a data-heavy analytics view into a clean fintech-style monitoring dashboard. Remove analytics clutter (AI uses `/api/analytics` directly), keep operational controls, add an equity curve chart.

## Layout: Two-Column Split

Full-width header spans both columns. Below it, a two-column CSS grid:

### Left Column (~60%) — "What's happening now"
1. **Status card** — key/value table (Mode, Market, Time left, BTC price, Poly prices, Model, Candles, Why no entry?)
2. **Open Trade card** — current position details with unrealized PnL
3. **Trades table** — full table with filters (limit, reason, side, losses-only)

### Right Column (~40%) — "How am I doing"
1. **KPI tiles** — 2x2 grid: Balance, PnL Today, PnL Yesterday, Win Rate
2. **Ledger Summary card** — mode/balance/realized/config summary
3. **Equity Curve chart** — Chart.js line chart showing cumulative PnL across all closed trades

### Responsive
- Below 900px: columns stack vertically, right column content appears first (KPIs at top)

## Visual Style — Clean Fintech

- **Background:** Dark theme, toned-down gradients (less blue/green glow, more neutral)
- **Cards:** Elevated with soft `box-shadow: 0 2px 8px rgba(0,0,0,0.3)` instead of hard borders
- **Typography:** Monospace for all numbers/values; system sans for labels/headings
- **KPI values:** 28px (up from 22px)
- **Spacing:** 20px card padding, 16px gaps between cards
- **Color:** Green/red reserved for PnL values only; blue accent for interactive elements
- **Header:** Bottom border separator

## Removals

- All 17 analytics mini-tables (exit reason, phase, price, side source, time left, probability, liquidity, market volume, spread, edge, VWAP distance, RSI, hold time, MAE, MFE, side, rec action)
- Analytics overview section (`#analytics-overview`)
- 3 charts: PnL by Exit Reason, Entry Price Bucket, PnL Distribution
- All related JS: `renderGroupTable()`, `updateBarChart()`, `updatePnlHistogram()`, analytics fetch block, chart variables (`chartExit`, `chartEntryPrice`, `chartPnlHist`)
- All related CSS: `.analytics-grid`, `.mini-table` styles
- Related HTML element IDs and sections

## Kept

- Chart.js CDN (for equity curve)
- `chartEquity` + `updateEquityCurve()` (moved to right column)
- Header with trading controls (mode select, status pill, start/stop)
- KPI tiles (Balance, PnL Today, PnL Yesterday, Win Rate)
- Status section (key/value table)
- Open Trade section
- Ledger Summary section
- Trades table with filters
- All polling logic + oscillation guards (`_modeSwitchInFlight`, `_tradingToggleInFlight`, `_fetchInProgress`)

## Added

- Two-column CSS grid layout (`.dashboard-grid`)
- Refined card component styles (shadow-based depth)
- Equity curve positioned in right column below Ledger Summary

## Files Changed

| File | Change |
|------|--------|
| `src/ui/index.html` | New two-column layout structure; remove all analytics sections |
| `src/ui/style.css` | Full rewrite for two-column fintech aesthetic |
| `src/ui/script.js` | Remove analytics rendering code; keep equity curve + status/trades logic |

## No Backend Changes

All API endpoints remain as-is. The `/api/analytics` endpoint stays available for programmatic/AI access.

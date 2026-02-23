# Dashboard Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Redesign the trading dashboard from an analytics-heavy layout into a clean two-column fintech monitoring dashboard with an equity curve chart.

**Architecture:** Three-file change — `index.html` gets a new two-column grid structure with analytics sections removed, `style.css` is fully rewritten for the fintech aesthetic, and `script.js` drops all analytics rendering while keeping equity curve, status polling, and trade table logic intact. No backend changes.

**Tech Stack:** HTML/CSS/JS, Chart.js (CDN, already loaded — kept for equity curve only)

---

### Task 1: Rewrite `index.html` — New Two-Column Layout

**Files:**
- Modify: `src/ui/index.html` (full rewrite of `<body>` content)

**Step 1: Replace the full HTML body content**

Replace the entire content of `src/ui/index.html` with the new two-column layout. The structure:

- Full-width header (kept as-is with trading controls)
- `.dashboard-grid` CSS grid container:
  - `.col-left` (~60%): Status card → Open Trade card → Trades table with filters
  - `.col-right` (~40%): KPI tiles (2×2 grid) → Ledger Summary card → Equity Curve chart

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Polymarket BTC 5m Assistant</title>
    <link rel="stylesheet" href="style.css" />
    <script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js"></script>
  </head>
  <body>
    <div class="container">
      <!-- ── Header ── -->
      <div class="header">
        <div>
          <h1>Polymarket BTC 5m Assistant</h1>
          <div class="sub">Unified trading engine • Paper &amp; Live</div>
        </div>
        <div class="trading-controls">
          <select id="mode-select" class="mode-select">
            <option value="paper">Paper</option>
            <option value="live">Live</option>
          </select>
          <span id="trading-status" class="trading-status status--stopped">STOPPED</span>
          <button id="start-trading" class="action-button action-button--start">Start Trading</button>
          <button id="stop-trading" class="action-button action-button--stop" disabled>Stop Trading</button>
        </div>
      </div>

      <!-- ── Two-Column Dashboard Grid ── -->
      <div class="dashboard-grid">
        <!-- Left Column: What's happening now -->
        <div class="col-left">
          <div class="card">
            <div class="card-title">Status</div>
            <div id="status-message">Loading...</div>
          </div>

          <div class="card">
            <div class="card-title">Open Trade</div>
            <div id="open-trade">No open trade.</div>
          </div>

          <div class="card">
            <div class="card-title">
              <div class="row-between">
                <span>Trades</span>
                <div class="filters">
                  <label>
                    Show
                    <select id="trades-limit">
                      <option value="25">25</option>
                      <option value="50" selected>50</option>
                      <option value="100">100</option>
                      <option value="200">200</option>
                    </select>
                  </label>
                  <label>
                    Reason
                    <select id="trades-reason">
                      <option value="">All</option>
                    </select>
                  </label>
                  <label>
                    Side
                    <select id="trades-side">
                      <option value="">All</option>
                      <option value="UP">UP</option>
                      <option value="DOWN">DOWN</option>
                    </select>
                  </label>
                  <label>
                    <input type="checkbox" id="trades-only-losses" /> losses only
                  </label>
                </div>
              </div>
            </div>
            <table id="recent-trades-table">
              <thead>
                <tr>
                  <th>Entry Time</th>
                  <th>Exit Time</th>
                  <th>Side</th>
                  <th>Entry Price</th>
                  <th>Exit Price</th>
                  <th>PnL</th>
                  <th>Status</th>
                  <th>Exit Reason</th>
                </tr>
              </thead>
              <tbody id="recent-trades-body">
                <tr>
                  <td colspan="8">Loading recent trades...</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>

        <!-- Right Column: How am I doing -->
        <div class="col-right">
          <div class="kpi-grid">
            <div class="kpi">
              <div class="kpi-label">Balance</div>
              <div class="kpi-value" id="kpi-balance">—</div>
              <div class="kpi-sub" id="kpi-realized">Realized: —</div>
            </div>
            <div class="kpi">
              <div class="kpi-label">PnL (Today)</div>
              <div class="kpi-value" id="kpi-pnl-today">—</div>
              <div class="kpi-sub" id="kpi-trades-today">Trades: —</div>
            </div>
            <div class="kpi">
              <div class="kpi-label">PnL (Yesterday)</div>
              <div class="kpi-value" id="kpi-pnl-yesterday">—</div>
              <div class="kpi-sub" id="kpi-trades-yesterday">Trades: —</div>
            </div>
            <div class="kpi">
              <div class="kpi-label">Win Rate</div>
              <div class="kpi-value" id="kpi-winrate">—</div>
              <div class="kpi-sub" id="kpi-profit-factor">PF: —</div>
            </div>
          </div>

          <div class="card">
            <div class="card-title">Ledger Summary</div>
            <div id="ledger-summary">Loading summary...</div>
          </div>

          <div class="card">
            <div class="card-title">Equity Curve</div>
            <canvas id="chart-equity" height="200"></canvas>
          </div>
        </div>
      </div>
    </div>

    <script src="script.js"></script>
  </body>
</html>
```

Key changes from the current file:
- **Removed:** `.section--dashboard` wrapper, `.chart-grid` with 4 charts (lines 52-69), entire Analytics section with 17 mini-tables (lines 72-196), old Status/Open Trade/Ledger Summary `.section` wrappers (lines 198-211)
- **Added:** `.dashboard-grid` > `.col-left` + `.col-right` two-column grid
- **Moved:** KPI tiles into `.col-right`, equity curve canvas into `.col-right` below Ledger Summary
- **Kept:** Header with trading controls, all element IDs unchanged, trades table with filters

**Step 2: Verify the HTML is valid**

Open `src/ui/index.html` in a browser or run a quick visual check. All existing element IDs (`status-message`, `open-trade`, `ledger-summary`, `kpi-balance`, `kpi-pnl-today`, `kpi-pnl-yesterday`, `kpi-winrate`, `chart-equity`, `recent-trades-body`, `trades-limit`, `trades-reason`, `trades-side`, `trades-only-losses`, `mode-select`, `trading-status`, `start-trading`, `stop-trading`) must be present exactly once.

**Step 3: Commit**

```bash
git add src/ui/index.html
git commit -m "feat(ui): rewrite HTML to two-column dashboard layout

Remove all 17 analytics mini-tables, 3 analytics charts, and analytics
overview section. Restructure into two-column grid: left column for
status/trade/trades-table, right column for KPIs/ledger/equity-curve."
```

---

### Task 2: Rewrite `style.css` — Clean Fintech Aesthetic

**Files:**
- Modify: `src/ui/style.css` (full rewrite)

**Step 1: Replace the full CSS file**

Replace the entire content of `src/ui/style.css`. The new stylesheet:

- **Tones down gradients** — subtle radials, more neutral dark bg
- **Shadow-based cards** — `box-shadow: 0 2px 8px rgba(0,0,0,0.3)` instead of hard borders
- **KPI values bumped to 28px** (from 22px)
- **20px card padding, 16px gaps**
- **Two-column grid** — `.dashboard-grid` with `3fr 2fr` columns
- **Responsive** — stacks below 900px, right column first
- **Removes** — `.analytics-grid`, `.mini-table`, `.chart-grid`, `.section--dashboard` styles

```css
:root {
  --bg: #0d1117;
  --surface: #161b22;
  --surface2: #1c2333;
  --text: #e6edf3;
  --muted: #8b949e;
  --border: rgba(255,255,255,0.06);
  --accent: #58a6ff;
  --good: #3fb950;
  --bad: #f85149;
  --warn: #d29922;
  --mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
  --sans: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Helvetica, Arial, "Apple Color Emoji", "Segoe UI Emoji";
  --card-shadow: 0 2px 8px rgba(0,0,0,0.3);
  --card-radius: 12px;
}

*,
*::before,
*::after {
  box-sizing: border-box;
}

body {
  font-family: var(--sans);
  background: var(--bg);
  color: var(--text);
  margin: 0;
  padding: 20px;
  line-height: 1.5;
}

/* ── Container ── */
.container {
  max-width: 1440px;
  width: 100%;
  margin: 0 auto;
}

/* ── Header ── */
.header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding-bottom: 16px;
  margin-bottom: 16px;
  border-bottom: 1px solid var(--border);
}

h1 {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  letter-spacing: -0.01em;
}

.sub {
  margin-top: 2px;
  color: var(--muted);
  font-size: 13px;
}

/* ── Trading Controls ── */
.trading-controls {
  display: flex;
  align-items: center;
  gap: 10px;
}

.mode-select {
  font-family: var(--mono);
  font-size: 12px;
  font-weight: 700;
  padding: 6px 12px;
  border-radius: 8px;
  background: var(--surface);
  color: var(--accent);
  border: 1px solid rgba(88,166,255,0.25);
  cursor: pointer;
  outline: none;
}
.mode-select:hover { border-color: rgba(88,166,255,0.5); }
.mode-select option { background: var(--surface); color: var(--text); }

.trading-status {
  font-family: var(--mono);
  font-size: 12px;
  font-weight: 700;
  padding: 6px 14px;
  border-radius: 999px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
.status--active {
  color: var(--good);
  background: rgba(63,185,80,0.1);
  border: 1px solid rgba(63,185,80,0.25);
}
.status--stopped {
  color: var(--bad);
  background: rgba(248,81,73,0.1);
  border: 1px solid rgba(248,81,73,0.25);
}

.action-button {
  font-family: var(--sans);
  font-size: 13px;
  font-weight: 600;
  padding: 8px 18px;
  border-radius: 8px;
  border: 1px solid var(--border);
  cursor: pointer;
  transition: background 0.15s, opacity 0.15s;
}
.action-button:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}
.action-button--start {
  background: rgba(63,185,80,0.12);
  color: var(--good);
  border-color: rgba(63,185,80,0.25);
}
.action-button--start:hover:not(:disabled) {
  background: rgba(63,185,80,0.22);
}
.action-button--stop {
  background: rgba(248,81,73,0.12);
  color: var(--bad);
  border-color: rgba(248,81,73,0.25);
}
.action-button--stop:hover:not(:disabled) {
  background: rgba(248,81,73,0.22);
}

/* ── Two-Column Grid ── */
.dashboard-grid {
  display: grid;
  grid-template-columns: 3fr 2fr;
  gap: 16px;
  align-items: start;
}

.col-left,
.col-right {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

/* ── Cards ── */
.card {
  background: var(--surface);
  border-radius: var(--card-radius);
  padding: 20px;
  box-shadow: var(--card-shadow);
}

.card-title {
  color: var(--muted);
  font-size: 12px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 12px;
}

/* ── KPI Tiles (2×2 grid in right column) ── */
.kpi-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
}

.kpi {
  background: var(--surface);
  border-radius: var(--card-radius);
  padding: 20px;
  box-shadow: var(--card-shadow);
}

.kpi-label {
  color: var(--muted);
  font-size: 12px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.kpi-value {
  font-family: var(--mono);
  font-size: 28px;
  font-weight: 700;
  margin-top: 8px;
  line-height: 1.2;
}

.kpi-sub {
  color: var(--muted);
  font-family: var(--mono);
  font-size: 12px;
  margin-top: 6px;
}

/* ── Status key/value table ── */
.kv-table {
  width: 100%;
  border-collapse: collapse;
}

.kv-table td {
  padding: 8px 0;
  border-bottom: 1px solid var(--border);
  font-family: var(--mono);
  font-size: 13px;
  vertical-align: top;
}

.kv-table td.k {
  width: 160px;
  color: var(--muted);
  font-weight: 600;
  white-space: nowrap;
  padding-right: 16px;
}

.kv-table td.v {
  color: var(--text);
  word-break: break-word;
}

.kv-table a {
  color: var(--accent);
  text-decoration: none;
}
.kv-table a:hover {
  text-decoration: underline;
}

/* ── Open Trade / Ledger Summary (monospace blocks) ── */
#open-trade,
#ledger-summary {
  font-family: var(--mono);
  font-size: 13px;
  white-space: pre-wrap;
  word-wrap: break-word;
  line-height: 1.6;
}

#open-trade.closed {
  color: var(--muted);
}

/* ── Equity Curve chart ── */
.card canvas {
  display: block;
  width: 100% !important;
  max-height: 240px;
}

/* ── Trades Table ── */
.row-between {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  flex-wrap: wrap;
}

.filters {
  display: flex;
  gap: 10px;
  align-items: center;
  flex-wrap: wrap;
  color: var(--muted);
  font-size: 12px;
}

.filters select,
.filters input[type="checkbox"] {
  margin-left: 6px;
}

.filters select {
  background: var(--surface2);
  border: 1px solid var(--border);
  color: var(--text);
  border-radius: 6px;
  padding: 4px 8px;
  font-family: var(--mono);
  font-size: 12px;
}

#recent-trades-table {
  width: 100%;
  border-collapse: collapse;
  margin-top: 4px;
}

#recent-trades-table th,
#recent-trades-table td {
  padding: 8px 10px;
  font-family: var(--mono);
  font-size: 12px;
  vertical-align: top;
  text-align: left;
  border-bottom: 1px solid var(--border);
}

#recent-trades-table th {
  color: var(--muted);
  font-weight: 600;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

#recent-trades-table tbody tr:hover {
  background: rgba(255,255,255,0.02);
}

/* ── PnL colors ── */
.positive { color: var(--good); }
.negative { color: var(--bad); }

/* ── Responsive: stack below 900px, right column first ── */
@media (max-width: 900px) {
  .header {
    flex-direction: column;
    align-items: flex-start;
  }

  .dashboard-grid {
    grid-template-columns: 1fr;
  }

  .col-right {
    order: -1;
  }

  .kpi-grid {
    grid-template-columns: 1fr 1fr;
  }
}

@media (max-width: 500px) {
  body { padding: 12px; }

  .kpi-grid {
    grid-template-columns: 1fr;
  }

  .trading-controls {
    flex-wrap: wrap;
  }
}
```

Key differences from the current file:
- **Removed:** `.section`, `.section--dashboard`, `.chart-grid`, `.analytics-grid`, `.mini-table`, `#analytics-overview`, `.pill` styles
- **Changed:** Background from radial gradients to flat `#0d1117`; cards use `box-shadow` instead of `border`; KPI value size from 22px to 28px; spacing from 10-12px to 16-20px; color palette slightly adjusted (GitHub-style dark)
- **Added:** `.dashboard-grid`, `.col-left`, `.col-right`, responsive stacking at 900px (right column first)

**Step 2: Commit**

```bash
git add src/ui/style.css
git commit -m "feat(ui): rewrite CSS for clean fintech two-column layout

Shadow-based card depth, 28px KPI values, neutral dark bg, 16px gaps.
Remove analytics-grid and mini-table styles."
```

---

### Task 3: Clean Up `script.js` — Remove Analytics Code

**Files:**
- Modify: `src/ui/script.js`

This is the most delicate task. We remove all analytics rendering code while preserving every line of: trading controls, oscillation guards, status polling, open trade, ledger summary, KPI updates, equity curve, trades table, and filter logic.

**Step 1: Remove analytics element references (lines 88-106)**

Delete these 19 lines:

```javascript
  // Analytics elements
  const analyticsOverviewDiv = document.getElementById('analytics-overview');
  const analyticsByExitBody = document.getElementById('analytics-by-exit');
  const analyticsByPhaseBody = document.getElementById('analytics-by-phase');
  const analyticsByPriceBody = document.getElementById('analytics-by-price');
  const analyticsByInferredBody = document.getElementById('analytics-by-inferred');
  const analyticsByTimeLeftBody = document.getElementById('analytics-by-timeleft');
  const analyticsByProbBody = document.getElementById('analytics-by-prob');
  const analyticsByLiqBody = document.getElementById('analytics-by-liq');
  const analyticsByMktVolBody = document.getElementById('analytics-by-mktvol');
  const analyticsBySpreadBody = document.getElementById('analytics-by-spread');
  const analyticsByEdgeBody = document.getElementById('analytics-by-edge');
  const analyticsByVwapDistBody = document.getElementById('analytics-by-vwapdist');
  const analyticsByRsiBody = document.getElementById('analytics-by-rsi');
  const analyticsByHoldBody = document.getElementById('analytics-by-hold');
  const analyticsByMaeBody = document.getElementById('analytics-by-mae');
  const analyticsByMfeBody = document.getElementById('analytics-by-mfe');
  const analyticsBySideBody = document.getElementById('analytics-by-side');
  const analyticsByRecBody = document.getElementById('analytics-by-rec');
```

**Step 2: Remove the 3 analytics chart variables (lines 142-144)**

Delete these 3 lines:

```javascript
  let chartExit = null;
  let chartEntryPrice = null;
  let chartPnlHist = null;
```

Keep `let chartEquity = null;` (line 141).

**Step 3: Remove analytics chart initialization (lines 200-226)**

Delete the chart init blocks for `chartExit`, `chartEntryPrice`, and `chartPnlHist` inside `ensureCharts()`. These are the three `if (exitEl && !chartExit)`, `if (entryEl && !chartEntryPrice)`, and `if (histEl && !chartPnlHist)` blocks.

Keep the equity chart init block (`if (equityEl && !chartEquity)`).

**Step 4: Remove `updateBarChart()` function (lines 228-235)**

Delete entirely:

```javascript
  const updateBarChart = (chart, rows, { maxBars = 12 } = {}) => {
    if (!chart || !rows || !Array.isArray(rows)) return;
    const r = rows.slice(0, maxBars);
    chart.data.labels = r.map(x => x.key);
    chart.data.datasets[0].data = r.map(x => (typeof x.pnl === 'number' ? x.pnl : 0));
    chart.data.datasets[0].backgroundColor = r.map(x => ((x.pnl ?? 0) >= 0) ? 'rgba(46,229,157,0.55)' : 'rgba(255,92,122,0.55)');
    chart.update('none');
  };
```

**Step 5: Remove `updatePnlHistogram()` function (lines 266-292)**

Delete entirely:

```javascript
  const updatePnlHistogram = (trades, { limit = 200, bins = 18 } = {}) => {
    ...
  };
```

**Step 6: Remove `renderGroupTable()` function (lines 301-314)**

Delete entirely:

```javascript
  const renderGroupTable = (tbody, rows) => {
    ...
  };
```

**Step 7: Remove `lastAnalyticsCache` variable (line 317)**

Delete:

```javascript
  let lastAnalyticsCache = null;
```

Keep `let lastTradesCache = [];` and `let lastStatusCache = null;`.

**Step 8: Remove the entire analytics fetch block (lines 589-661)**

Inside `_fetchDataInner()`, delete the entire `// ---- analytics ----` try/catch block. This is the block that:
- Fetches `/api/analytics`
- Calls `renderGroupTable()` for all 17 tables
- Calls `updateBarChart()` for exit and entry-price charts
- Updates `analyticsOverviewDiv`

**Step 9: Remove `updatePnlHistogram()` call (line 724)**

Inside the trades fetch block, delete:

```javascript
      updatePnlHistogram(lastTradesCache, { limit: 200, bins: 18 });
```

**Step 10: Update chart colors to match new CSS palette**

In the `chartColors` object, update to match the new CSS variables:

```javascript
  const chartColors = {
    good: '#3fb950',
    bad: '#f85149',
    accent: '#58a6ff',
    muted: 'rgba(230,237,243,0.4)',
    grid: 'rgba(255,255,255,0.06)'
  };
```

**Step 11: Verify the final `script.js` has these sections intact**

After all removals, the file should contain (in order):
1. DOM element refs: `statusMessage`, `openTradeDiv`, `ledgerSummaryDiv`, KPI elements, trading controls, `recentTradesBody`, filter elements
2. `updateTradingStatus()` function
3. `_tradingToggleInFlight` guard + start/stop button handlers
4. `_modeSwitchInFlight` guard + mode select handler
5. Formatting helpers (`formatCurrency`, `formatPercentage`, `formatCents`, `dayKey`, etc.)
6. `chartEquity` variable + `chartColors` + `ensureCharts()` (equity only) + `updateEquityCurve()`
7. `setKpi()` helper
8. `lastTradesCache`, `lastStatusCache` variables
9. `renderTradesTable()`, `refreshReasonFilter()` functions
10. `_fetchInProgress` guard + `fetchData()` + `_fetchDataInner()`
11. Inside `_fetchDataInner()`: status fetch → open trade → ledger summary → trades fetch (no analytics)
12. Filter event listeners
13. `fetchData()` + `setInterval(fetchData, 1500)`

**Step 12: Commit**

```bash
git add src/ui/script.js
git commit -m "feat(ui): remove all analytics rendering from script.js

Remove 17 analytics element refs, 3 chart variables, renderGroupTable,
updateBarChart, updatePnlHistogram, analytics fetch block, and
lastAnalyticsCache. Keep equity curve, status, trades, KPIs, and all
polling/oscillation guards intact."
```

---

### Task 4: Manual Smoke Test

**Step 1: Start the dev server**

```bash
cd /Users/elijahprince/Dev/polymarket-btc-5m-assistant
node --max-old-space-size=1024 src/index.js
```

**Step 2: Verify in browser**

Open `http://localhost:3000` (or the configured port) and check:

- [ ] Two-column layout visible (left wider, right narrower)
- [ ] Header spans full width with mode select, status pill, start/stop buttons
- [ ] Left column: Status card (key/value table), Open Trade card, Trades table with working filters
- [ ] Right column: 4 KPI tiles (2×2), Ledger Summary, Equity Curve chart
- [ ] No analytics tables or charts anywhere
- [ ] KPI values display correctly (Balance, PnL Today, PnL Yesterday, Win Rate)
- [ ] Equity curve renders (or shows "No data yet" if no trades)
- [ ] Mode switching works without oscillation
- [ ] Start/Stop trading works without oscillation
- [ ] Responsive: resize below 900px, right column stacks on top

**Step 3: Check browser console**

No errors should appear. Specifically verify:
- No `getElementById` returning null for expected elements
- No Chart.js errors
- Network requests to `/api/status` and `/api/trades` succeeding (no `/api/analytics` request)

**Step 4: Final commit (if any tweaks needed)**

```bash
git add -A
git commit -m "fix(ui): post-redesign tweaks"
```

---

### Task 5: Update Changelog

**Files:**
- Modify: `changelog.md`

**Step 1: Add entry under `### 2026-02-22`**

Add a new section at the top of the 2026-02-22 entries:

```markdown
#### Dashboard Redesign
- **Full UI redesign** from analytics-heavy layout to clean two-column fintech monitoring dashboard.
- **Left column** (~60%): Status card (key/value table), Open Trade card, Trades table with filters.
- **Right column** (~40%): KPI tiles (2×2 grid: Balance, PnL Today, PnL Yesterday, Win Rate), Ledger Summary, Equity Curve chart.
- **Removed:** All 17 analytics mini-tables, analytics overview section, 3 analytics charts (PnL by Exit Reason, Entry Price Bucket, PnL Distribution), and all related JS rendering code (`renderGroupTable()`, `updateBarChart()`, `updatePnlHistogram()`).
- **Visual refresh:** Shadow-based card depth, 28px KPI values, neutral dark background, 16px spacing. Green/red reserved for PnL values only.
- **Responsive:** Below 900px columns stack vertically with KPIs on top.
- **No backend changes.** The `/api/analytics` endpoint remains available for programmatic/AI access.
```

**Step 2: Commit**

```bash
git add changelog.md
git commit -m "docs: add dashboard redesign to changelog"
```

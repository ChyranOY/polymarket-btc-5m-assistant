# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Polymarket BTC 5m Assistant** is a high-frequency trading bot for Polymarket's 5-minute BTC Up/Down contracts. It monitors BTC price movements, calculates trading signals using technical indicators (RSI, MACD, VWAP), and executes trades in both **paper trading mode** (simulated) and **live trading mode** (CLOB via Polymarket's API).

### Key Capabilities
- **Dual-mode trading**: Paper mode (simulated ledger) + Live mode (real CLOB execution)
- **Multi-timeframe signals**: BTC spot price (Kraken/Chainlink/Coinbase) + Polymarket contract prices
- **Dynamic position sizing**: Bankroll-based stake sizing with min/max bounds
- **Comprehensive indicators**: RSI (with slope detection), MACD (with histogram), VWAP (with liquidity fallback)
- **Safety guardrails**: Circuit breaker (consecutive loss cooldown), position tracking (MFE/MAE), daily loss limits
- **Dashboard UI**: Real-time status, live/paper ledger, trade history, KPIs, equity curve

## Architecture

### Clean Architecture Layers

```
┌─ Domain Layer ─────────────────────────────────────────────────────┐
│  Pure functions (no side effects) for trading decisions.            │
│  • entryGate.js      — 24 entry blockers (risk, time, indicator)   │
│  • exitEvaluator.js  — 8 exit conditions (rollover, prob flip, etc) │
│  • sizing.js         — Dynamic trade size calculation              │
└────────────────────────────────────────────────────────────────────┘

┌─ Application Layer ────────────────────────────────────────────────┐
│  Orchestration & state management. No external I/O (except calls   │
│  to executors and signals).                                         │
│  • TradingEngine.js      — Main loop orchestrator                   │
│  • TradingState.js       — MFE/MAE tracking, circuit breaker        │
│  • ModeManager.js        — Paper/Live toggle (not UI-driven)        │
│  • ExecutorInterface.js  — Abstract executor contract               │
└────────────────────────────────────────────────────────────────────┘

┌─ Infrastructure Layer ─────────────────────────────────────────────┐
│  External I/O (API calls, DB, file system, WebSockets).            │
│  • executors/PaperExecutor.js    — Simulated fills on ledger        │
│  • executors/LiveExecutor.js     — Real CLOB execution              │
│  • polymarket.js                 — Gamma API + CLOB client          │
│  • kraken.js, chainlink.js, etc  — Price feeds                      │
│  • ledger.js                     — Paper trading state persistence  │
└────────────────────────────────────────────────────────────────────┘

┌─ Presentation Layer ──────────────────────────────────────────────┐
│  UI & monitoring.                                                   │
│  • ui/index.html  — Two-column dashboard (status, trades, KPIs)    │
│  • ui/script.js   — Real-time polling, mode/trading toggles        │
│  • ui/server.js   — Express routes (/api/status, /api/trades, etc) │
└────────────────────────────────────────────────────────────────────┘
```

### Key Design Patterns

1. **Executor Abstraction** — `PaperExecutor` and `LiveExecutor` implement `OrderExecutor` interface. Swap at runtime via `ModeManager`.
2. **First-Poll-Only Sync** — Mode & trading status synced from server only on page load; then exclusively controlled by user actions (buttons/dropdown). Polling never overwrites these.
3. **Instance Locking** — Frontend locks to one server `_instanceId`; drops responses from other instances to prevent multi-instance oscillation (critical in production with load balancers).
4. **Circuit Breaker** — After 3 consecutive CLOB failures, halt all CLOB requests for exponential backoff (5s → 10s → 20s → … → 60s cap).

## Common Commands

```bash
# Start trading (server + UI on localhost:3000)
npm start

# Run tests (node:test native runner)
npm test

# Environment config
cp .env.example .env
# Edit .env for Polymarket API key, RPC endpoints, trading params
```

## Key File Locations & Responsibilities

| Path | Purpose |
|------|---------|
| `src/index.js` | Entry point: data provider setup, indicator loop, signal generation, main engine tick |
| `src/config.js` | Centralized config (indicators, thresholds, API endpoints, paper trade params) |
| `src/application/TradingEngine.js` | Unified engine: orchestrates entry/exit, calls executors, tracks state |
| `src/application/TradingState.js` | MFE/MAE tracking, circuit breaker, daily PnL, grace windows for max-loss |
| `src/application/ModeManager.js` | Paper/Live mode toggle (synced to config, updates global executors) |
| `src/domain/entryGate.js` | 24 entry blockers (e.g., "trading disabled", "insufficient probability", "out of hours") |
| `src/domain/exitEvaluator.js` | Exit conditions (max loss, profit target, probability flip, time-based rollover) |
| `src/infrastructure/executors/PaperExecutor.js` | Simulated fills: updates ledger, tracks open trade state |
| `src/infrastructure/executors/LiveExecutor.js` | Real CLOB execution: submits orders, fetches fills, manages approvals/fees |
| `src/data/polymarket.js` | Gamma API (market discovery) + CLOB client initialization |
| `src/ui/server.js` | Express API: `/api/status` (engine state), `/api/trading/start|stop`, `/api/mode`, `/api/trades` |
| `src/ui/script.js` | Frontend: 1.5s polling loop, mode dropdown, Start/Stop buttons, instance locking |
| `src/paper_trading/ledger.js` | Paper trade state (JSON file), entry/exit recording, realized/unrealized PnL |
| `src/services/statusService.js` | Assembles `/api/status` response: engine state + UI diagnostics |

## Frontend Architecture (UI)

### State Management
- **Mode dropdown** (`#mode-select`) — User selects PAPER/LIVE. POST to `/api/mode`, stored in `ModeManager`.
- **Trading pills** (`#start-trading`, `#stop-trading`, `#trading-status`) — User click → POST `/api/trading/start|stop` → engine updates `tradingEnabled`.
- **Polling loop** (1.5s) — Fetches `/api/status` + `/api/trades`, renders status table, trades table, KPIs, equity chart.

### Oscillation Prevention (Multi-Layer Defense)

1. **Instance Locking** — Frontend locks to first server `_instanceId`. Drops responses from other instances for ~7.5s (5 polls), then switches if original instance dies.
2. **First-Poll-Only Sync** — Mode & trading status only synced from server on first poll; then exclusively user-controlled.
3. **Dropdown as Source of Truth** — All rendering decisions (mode-dependent UI branches, trades endpoint selection) read from dropdown value, not server response.
4. **Entry Blocker Filtering** — When user enables trading locally, filter out stale "Trading disabled" blocker from server response (handles multi-instance races).

### Key Rendering Flows

- **Status Table** — Reads from `statusData` (server response), but uses **dropdown's mode** for conditional branches (open trade panel).
- **Trades Table** — Uses **dropdown's mode** to select trades endpoint (`/api/paper|live/trades`).
- **KPIs** — Mode-dependent: PAPER shows win rate/profit factor; LIVE shows collateral balance + limits.
- **"Why no entry?"** — Displays entry blockers from `entryDebug`; filters "Trading disabled" if user enabled trading locally.

## Signal Flow (per tick)

```
1. Fetch price data (Kraken/Chainlink/Coinbase)
2. Compute indicators (RSI, MACD, VWAP)
3. Fetch Polymarket snapshot (market prices, time left)
4. Build signals object (rec, probabilities, edges)
5. TradingEngine.processSignals(signals)
   a. Check entry blockers → update entryDebug
   b. If eligible, compute trade size
   c. Place order via executor (Paper or Live)
   d. Track MFE/MAE, record closes, update circuit breaker
6. Return success/failure reason
7. Update UI status + trades on next poll
```

## Configuration

Key environment variables (see `src/config.js`):

| Variable | Purpose | Example |
|----------|---------|---------|
| `POLYMARKET_SLUG` | Target market slug | `btc-up-or-down-5m` |
| `STARTING_BALANCE` | Paper trade bankroll | `1000` |
| `STAKE_PCT` | Position size as % of balance | `0.08` (8%) |
| `MIN_PROB_MID`, `EDGE_MID` | Entry thresholds | `0.53`, `0.03` |
| `PAPER_TRADING_ENABLED` | Enable paper mode | `true` |
| `FUNDER_ADDRESS` | Live trading funder (CLOB) | (set by user for live trading) |
| `POLYGON_RPC_URL` | Chainlink BTC feed | `https://polygon-rpc.com` |

## Testing

Run `npm test` (uses Node.js native `test` runner).

Example test structure:
```javascript
import test from 'node:test';
import assert from 'node:assert';

test('entryGate: market closed blocker', async (t) => {
  const blockers = computeEntryBlockers({ timeLeftMin: -1 }, {});
  assert(blockers.blockers.some(b => b.includes('closed')));
});
```

## Known Gotchas & Patterns

1. **Mode vs. Trading State** — Mode (PAPER/LIVE) and trading enabled (ACTIVE/STOPPED) are **independent**. User can be in LIVE mode but trading disabled.

2. **Multi-Instance Production** — DigitalOcean app platform spins up multiple Node processes behind a load balancer. POSTs may go to one instance, polls to another. **Instance locking + first-poll-only sync** solve this.

3. **Entry/Exit Asymmetry** — Entry blockers are recalculated every tick (conservative). Exit evaluation only runs if entry was eligible in a previous state (prevents thrashing).

4. **MFE/MAE Persistence** — Per-position, reset when position closes. Enables grace-window logic for max-loss recovery.

5. **Circuit Breaker** — Module-level in `polymarket.js` for CLOB requests (not order execution). Prevents TLS hang-ups from cascading.

6. **Paper Ledger** — JSON file in `data/paper_trading/ledger.json`. Survives restarts. Backup auto-created on write.

## Debugging Tips

- **Server logs** — Shows rec action, mode, signal reasons, blockers, exits.
- **UI console** — Instance locking debug logs prefixed `[UI]`.
- **Entry status** — "Why no entry?" field shows current blockers (or "ELIGIBLE").
- **Status table** — Includes candle count, time left, market slug, entry phase debug info.
- **Trades endpoint** — Returns mode-dependent data; verify mode dropdown matches expectation.

## Git & Commit Patterns

- Prefix commits with fix/feat/refactor (e.g., "Fix oscillation from multi-instance server").
- Co-author with `Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>` for AI-generated changes.
- Update `CHANGELOG.md` after fixes/features (document root cause, solution, trade-offs).

## Performance Considerations

- **Heap cap** — Start script includes `--max-old-space-size=1024` (1 GB).
- **Polling interval** — 1s main loop; 1.5s UI polls (independent).
- **Trades cache** — `_cachedTrades` capped at 500 entries; auto-prunes `_lastExitAttemptMsByToken` when > 50 entries old.
- **Fetch timeouts** — 5s abort timeout on all CLOB fetches (prevent stuck TLS connections).

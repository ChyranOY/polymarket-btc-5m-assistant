# Changelog

All notable changes to Polymarket BTC 5m Assistant are documented in this file.

## [1.0.0] - 2026-02-23

First production-ready release. Combines four development phases into a unified, deployable trading system.

### Phase 1: Analytics Foundation

- **Trade Journal Enrichment**: Every trade now records 20+ indicator values at entry and exit (RSI, MACD, VWAP slope, spot price, spread, liquidity, entry gate snapshot)
- **Period Analytics**: Performance grouped by day, week, and trading session with win rate, profit factor, PnL, and trade count
- **Advanced Metrics**: Sharpe ratio, Sortino ratio, max drawdown (USD + percentage), confidence classification
- **Backtest Harness**: Pure-function backtester replays historical paper trades with modified thresholds; no network I/O
- **Grid Search Optimizer**: Tests parameter combinations (up to 10,000) and ranks by profit factor; safety valve prevents overfitting with 30-trade minimum per combination
- **Three-Tab Dashboard**: Dashboard (live status + trades), Analytics (period performance + drawdown chart), Optimizer (parameter tuning + apply/revert)

### Phase 2: Profitability Optimization

- **Segmented Performance Views**: Win rate and profit factor broken down by entry phase, trading session, and market regime (Oversold/Ranging/Overbought)
- **Suggestion Engine**: Analyzes blocker frequency data, maps blockers to threshold parameters, validates suggestions via backtest, and presents ranked suggestions with projected impact
- **Suggestion Cards UI**: Interactive cards with confidence indicators, current vs. suggested values, projected metrics, and one-click apply
- **Post-Apply Tracking**: Monitors actual performance after applying suggestions and flags underperforming changes

### Phase 3: Live Trading Hardening

- **Order Lifecycle State Machine**: Tracks every order through SUBMITTED -> PENDING -> FILLED -> MONITORING -> EXITED (or CANCELLED/TIMED_OUT/FAILED)
- **Retry Policy**: Classifies CLOB errors (NETWORK, RATE_LIMIT, SERVER, VALIDATION, UNKNOWN), retries with exponential backoff (1s -> 2s -> 4s), circuit breaker after 3 consecutive failures
- **Fee-Aware Sizing**: Subtracts estimated Polymarket fees from trade size so actual position matches intended risk exposure
- **Kill-Switch**: Absolute dollar daily loss limit with midnight PT reset, manual override with 10% additional buffer, re-trigger capability
- **Position Reconciliation**: Every-tick comparison of local position state vs. CLOB API; logs discrepancies without auto-correcting
- **Dashboard Integration**: Order lifecycle badges, kill-switch progress bar with override button, sync indicator dot

### Phase 4: Infrastructure & Monitoring

- **SQLite Persistence**: Trade history stored in structured SQLite database (better-sqlite3) with full schema for SQL-based analytics; automatic migration from JSON ledger; JSON remains as compatibility fallback
- **Webhook Alerting**: Slack and Discord adapters for critical event alerts (kill-switch, circuit breaker, order failure, crash recovery); fire-and-forget delivery with 60s deduplication per event type
- **Crash Recovery**: PID lock file for crash detection, JSON state file for critical state persistence (kill-switch, daily PnL, circuit breaker), automatic restoration on startup
- **Zero-Downtime Deployment**: File-based trading lock with heartbeat for instance coordination; graceful shutdown (SIGTERM handler drains open positions up to 5 minutes); enhanced /health endpoint for load balancer probes

### Phase 5: Integration & Polish

- **Integration Tests**: End-to-end test suite covering paper trading flow, live mock flow (with stubbed CLOB), and crash recovery flow
- **Dashboard Status Bar**: Compact 6-indicator status bar (Mode, Trading, Kill-switch, SQLite, Webhooks, Uptime) with graceful degradation for unconfigured features
- **SQLite Fallback Banner**: Dashboard shows subtle info banner when SQLite is unavailable, nudging user to install better-sqlite3
- **Production Pre-flight Script**: `npm run preflight` validates environment (tests, env vars, SQLite access, config sanity)
- **Startup Env Validation**: Required and optional env var validation on boot with clear log messages
- **NODE_ENV Production Defaults**: Stricter behavior in production (no debug logging, kill-switch required)
- **Documentation**: Full documentation suite (README rewrite, DEPLOYMENT.md operational runbook, CHANGELOG.md, CLAUDE.md update)

## [0.1.0] - Pre-release

Initial trading bot with paper trading, BTC price feeds, technical indicators, and basic web dashboard.

---
*Maintained by @krajekis*

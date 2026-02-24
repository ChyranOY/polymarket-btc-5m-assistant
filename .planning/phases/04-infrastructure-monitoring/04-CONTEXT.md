# Phase 4: Infrastructure & Monitoring - Context

**Gathered:** 2026-02-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Add operational reliability features — webhook alerting on critical events, crash recovery with state restoration, structured SQLite persistence replacing the JSON ledger, and zero-downtime deployment with instance coordination. This phase hardens the production infrastructure so the bot can run unattended with confidence.

Requirements covered: INFRA-05, INFRA-06, INFRA-07, INFRA-08.

What this phase does NOT include: new trading strategies, analytics changes, indicator development, UI redesign, multi-user support.

</domain>

<decisions>
## Implementation Decisions

### Webhook Alert Configuration & Delivery
- Critical events only — kill-switch activation, circuit breaker trip, ORDER_FAILED (after retries exhausted), process crash. High-signal alerts that don't get ignored.
- Env vars only for configuration — `WEBHOOK_URL` and `WEBHOOK_TYPE` (slack/discord) in `.env`. Matches existing config.js pattern. No UI settings panel.
- Fire-and-forget delivery with logging — attempt delivery, log failure to console, don't retry. Webhooks are notifications, not critical path. Never block the trading loop.
- Both Slack and Discord via adapter pattern — `formatForSlack()` and `formatForDiscord()`. Discord accepts Slack-compatible webhook format so minimal extra work.

### Crash Recovery & State Restoration
- Critical trading state only — recover kill-switch state (triggered/loss amount/override), daily PnL, circuit breaker state, open position tracking. Cooldowns and MFE/MAE can safely reset (they're short-lived).
- PID lock file for crash detection — write PID to `.pid` file on startup, remove on clean shutdown. On startup, if `.pid` exists with a dead process, assume crash occurred.
- Query CLOB on startup and reconcile in-flight orders — use Phase 3 reconciliation module. Resume monitoring if position exists on CLOB, mark as orphaned if not.
- JSON state file for persistence — write critical state to `state.json` on each significant state change (kill-switch trigger, circuit breaker trip, trade close). Human-readable, consistent with existing ledger pattern.

### Persistence Format & Migration
- SQLite for structured trade persistence — proper relational DB with queryable SQL, atomic writes, index support for time-range queries. Small binary, no external server. Standard for single-process apps.
- Full migration to SQLite — migrate all existing trades from JSON to SQLite. Switch all reads (analytics, backtest, suggestion engine) to SQLite. JSON ledger becomes backup only.
- Time-range + basic filter queries — get trades by date range, filter by outcome (win/loss), filter by mode (paper/live). Matches current usage patterns.
- Full enriched trade schema — store all 20+ indicator fields (rsiAtEntry, macdAtEntry, vwapAtEntry, etc.), entry gate evaluation, exit metadata. Future-proofs for SQL-based analytics without needing the JSON ledger.

### Zero-Downtime Deployment & Instance Coordination
- File-based lock for coordination — `trading.lock` file with instance ID + heartbeat timestamp. Only the lock holder can execute trades. New instance waits for lock release or stale heartbeat (>30s). Simple, no external dependencies.
- Graceful drain on SIGTERM — stop accepting new trades, wait for open position to close (up to 5 min timeout for 5m contracts), persist state, release lock, exit. New instance acquires lock and starts.
- HTTP health endpoint — `GET /health` returns 200 if server running and trading loop active. Includes basic diagnostics (uptime, last tick, mode). DigitalOcean uses this to verify readiness.
- Platform-agnostic with DO notes — generic file-based locking that works anywhere (local dev, DO, any VPS). Document DigitalOcean-specific config in deployment guide. Portable.

### Claude's Discretion
- Webhook payload format details (fields, formatting, color coding for Slack blocks/Discord embeds)
- SQLite schema column types and index design
- Migration script implementation (batch size, error handling, progress reporting)
- Which analytics/backtest code paths to migrate from JSON reads to SQLite reads
- Lock file path and heartbeat update frequency
- SIGTERM drain timeout specifics and edge cases
- Health endpoint response schema
- State file write debouncing (avoid excessive disk I/O on every tick)
- `better-sqlite3` vs `sql.js` vs Node built-in SQLite driver choice

</decisions>

<specifics>
## Specific Ideas

- Phase 3 structured failure events (`{ type: 'ORDER_FAILED', orderId, error, retryCount, timestamp }`) are ready for webhook consumption — webhook module should subscribe to these
- The existing circuit breaker in polymarket.js (module-level) already tracks consecutive failures — crash recovery should restore this count
- The existing `loadLedger()` / `saveLedger()` pattern in `ledger.js` provides the migration source — read all trades from JSON, insert into SQLite
- The existing frontend instance locking (`_instanceId` check, 7.5s timeout) provides client-side protection — the new server-side file lock adds backend protection
- The Phase 3 reconciliation module (`reconciliation.js`) can be reused for post-crash position recovery — same compare-and-flag logic
- The existing `analyticsService.js`, `backtester.js`, `backtestService.js`, `suggestionService.js` all call `loadLedger()` for trade data — these need to be migrated to SQLite reads
- `state.json` and `trading.lock` should be written to a configurable data directory (default `./data/`) alongside the SQLite DB

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 04-infrastructure-monitoring*
*Context gathered: 2026-02-23*

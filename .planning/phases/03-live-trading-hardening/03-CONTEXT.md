# Phase 3: Live Trading Hardening - Context

**Gathered:** 2026-02-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Make the live CLOB execution path production-ready with full order lifecycle management, position reconciliation, fee-aware sizing, graceful error recovery with retry logic, and a validated daily PnL kill-switch. This phase hardens the existing LiveExecutor into a reliable production system.

Requirements covered: LIVE-01, LIVE-02, LIVE-03, LIVE-04, LIVE-05.

What this phase does NOT include: webhook alerts (Phase 4 — INFRA-05), crash recovery (Phase 4 — INFRA-06), database persistence (Phase 4 — INFRA-07), zero-downtime deployment (Phase 4 — INFRA-08), new trading strategies, analytics changes.

</domain>

<decisions>
## Implementation Decisions

### Order Lifecycle States
- 30-second fill timeout — if order not filled within 30s, mark as TIMED_OUT and auto-cancel
- Accept partial fills — if any shares fill, accept the position at partial size, cancel unfilled remainder, log fill ratio
- Dashboard shows lifecycle state in real-time — Open Trade card displays current state (SUBMITTED/PENDING/FILLED/MONITORING/EXITED) with timestamps for each transition and a colored state indicator
- Auto-cancel stuck orders after timeout — submit CLOB cancel request automatically. If cancel fails, flag for manual intervention

### Reconciliation Behavior
- Reconcile every tick (~1 second) — frequent checks for real-money trading with 5-minute contracts
- Reconcile both positions and open orders — catches position tracking errors and order state mismatches
- Log + alert on discrepancy, do NOT auto-correct — log full details, flag in dashboard, make available for Phase 4 webhook alerts. Human investigates and fixes
- Dashboard shows sync status indicator — green (in sync), yellow (checking), red (discrepancy detected with details). Displayed in Live mode status card

### Failure & Retry Policy
- Two-layer protection: per-order retries (up to 3 attempts with 1s → 2s → 4s backoff, 30s cap) PLUS existing circuit breaker (3 consecutive failures across all requests, 60s cap)
- Retryable errors: network errors, timeouts, 5xx server errors, rate limits (429). Fatal errors: authentication failures (401/403), invalid parameters (400), insufficient funds (422)
- Structured failure events: each failed order (after all retries exhausted) creates a structured event { type: 'ORDER_FAILED', orderId, error, retryCount, timestamp } for Phase 4 webhook consumption. Also logged to server console

### Kill-Switch Rules
- Trigger: absolute dollar loss threshold (e.g., -$50 daily loss, configured via DAILY_LOSS_LIMIT)
- Reset: midnight Pacific time (consistent with dashboard analytics period grouping)
- Override: explicit dashboard button with confirmation dialog. Override is logged. Kill-switch can re-trigger if losses continue after override
- Scope: applies to BOTH paper and live modes — paper mode validates kill-switch behavior before going live

### Claude's Discretion
- Fee estimation calculation method (percentage-based, fixed, or lookup from CLOB API)
- Exact lifecycle state machine transitions and edge case handling
- Reconciliation comparison algorithm (which fields to compare, tolerance thresholds)
- Structured event schema details beyond the basic fields listed above
- Kill-switch override UI placement and confirmation dialog design
- How fee estimation integrates with the existing sizing.js position size calculation

</decisions>

<specifics>
## Specific Ideas

- The existing circuit breaker in polymarket.js (module-level, 5s → 10s → 20s → 60s) should be preserved — per-order retries add a layer below it, not replace it
- The existing TradingState daily PnL tracking and grace window should be extended, not rewritten
- Structured failure events should follow a pattern that Phase 4 webhooks can consume without parsing
- The reconciliation status indicator should be visible ONLY in Live mode (not paper mode where reconciliation doesn't apply)

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 03-live-trading-hardening*
*Context gathered: 2026-02-23*

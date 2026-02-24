# Phase 3: Live Trading Hardening - Research

**Researched:** 2026-02-23
**Domain:** CLOB order lifecycle, position reconciliation, error recovery, kill-switch
**Confidence:** HIGH

## Summary

Phase 3 hardens the existing LiveExecutor for production readiness. The codebase already has substantial live trading infrastructure: LiveExecutor (CLOB order placement/exit), FeeService (observability), ApprovalService (collateral/conditional token management), OrderManager (basic tracking), and a module-level circuit breaker in polymarket.js. The work is primarily extending and connecting existing patterns rather than building from scratch.

Key technical challenges: (1) order lifecycle state machine with timeout/cancel/partial-fill logic, (2) per-tick position reconciliation against CLOB API without auto-correction, (3) two-layer retry (per-order + existing circuit breaker), and (4) validating the daily PnL kill-switch end-to-end with dashboard override.

**Primary recommendation:** Extend existing classes (OrderManager for lifecycle, TradingState for kill-switch, LiveExecutor for reconciliation) and add new domain-layer pure functions for retry logic and fee-aware sizing. Keep the architecture clean: domain layer stays pure, infrastructure handles I/O.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- 30-second fill timeout — if order not filled within 30s, mark as TIMED_OUT and auto-cancel
- Accept partial fills — if any shares fill, accept the position at partial size, cancel unfilled remainder, log fill ratio
- Dashboard shows lifecycle state in real-time — Open Trade card displays current state (SUBMITTED/PENDING/FILLED/MONITORING/EXITED) with timestamps for each transition and a colored state indicator
- Auto-cancel stuck orders after timeout — submit CLOB cancel request automatically. If cancel fails, flag for manual intervention
- Reconcile every tick (~1 second) — frequent checks for real-money trading with 5-minute contracts
- Reconcile both positions and open orders — catches position tracking errors and order state mismatches
- Log + alert on discrepancy, do NOT auto-correct — log full details, flag in dashboard, make available for Phase 4 webhook alerts. Human investigates and fixes
- Dashboard shows sync status indicator — green (in sync), yellow (checking), red (discrepancy detected with details). Displayed in Live mode status card
- Two-layer protection: per-order retries (up to 3 attempts with 1s -> 2s -> 4s backoff, 30s cap) PLUS existing circuit breaker (3 consecutive failures across all requests, 60s cap)
- Retryable errors: network errors, timeouts, 5xx server errors, rate limits (429). Fatal errors: authentication failures (401/403), invalid parameters (400), insufficient funds (422)
- Structured failure events: each failed order (after all retries exhausted) creates a structured event { type: 'ORDER_FAILED', orderId, error, retryCount, timestamp } for Phase 4 webhook consumption. Also logged to server console
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

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| LIVE-01 | Full order lifecycle tracked from submission through fill to exit with status at each stage | OrderManager already exists with trackOrder(); extend with state machine (SUBMITTED->PENDING->FILLED->MONITORING->EXITED), timeout detection, partial fill handling |
| LIVE-02 | System reconciles CLOB position state with local tracking and flags discrepancies | LiveExecutor already fetches trades via client.getTrades(); add comparison logic against local OrderManager state, discrepancy logging, dashboard status indicator |
| LIVE-03 | Position sizing accounts for estimated fees before submitting orders | FeeService already computes feeImpact; integrate into sizing.js computeTradeSize() or LiveExecutor.openPosition() to subtract estimated fees before order |
| LIVE-04 | CLOB failures trigger automatic retry with exponential backoff and alert | Per-order retry wrapper around createAndPostOrder(); classifies errors as retryable vs fatal; emits structured failure events |
| LIVE-05 | Daily PnL kill-switch validated end-to-end (triggers correctly at threshold) | TradingState already tracks todayRealizedPnl and entryGate has blocker #25; add override mechanism, dashboard UI, midnight PT reset validation |
</phase_requirements>

## Standard Stack

### Core (Already in Codebase)
| Module | Location | Purpose | Status |
|--------|----------|---------|--------|
| LiveExecutor | src/infrastructure/executors/LiveExecutor.js | CLOB order execution | Extend |
| OrderManager | src/infrastructure/orders/OrderManager.js | Order tracking | Extend significantly |
| FeeService | src/infrastructure/fees/FeeService.js | Fee rate lookup + impact calc | Integrate with sizing |
| TradingState | src/application/TradingState.js | Daily PnL, circuit breaker, MFE/MAE | Extend for kill-switch |
| entryGate.js | src/domain/entryGate.js | Entry blockers (25 total) | Blocker #25 already exists |
| sizing.js | src/domain/sizing.js | Position size calculation | Add fee deduction |
| polymarket.js | src/data/polymarket.js | CLOB API + circuit breaker | Preserve circuit breaker |
| config.js | src/config.js | All thresholds | Add new config keys |

### No New Dependencies
This phase requires NO new npm packages. All work extends existing code using Node.js built-in capabilities.

## Architecture Patterns

### Pattern 1: Order Lifecycle State Machine
**What:** Finite state machine tracking order from submission to completion
**States:** SUBMITTED -> PENDING -> FILLED -> MONITORING -> EXITED (plus TIMED_OUT, CANCELLED, PARTIAL_FILL, FAILED)
**Implementation:** Extend OrderManager with state tracking, timestamps per transition, timeout detection

```javascript
// State transitions
const TRANSITIONS = {
  SUBMITTED: ['PENDING', 'TIMED_OUT', 'FAILED', 'CANCELLED'],
  PENDING:   ['FILLED', 'PARTIAL_FILL', 'TIMED_OUT', 'FAILED', 'CANCELLED'],
  FILLED:    ['MONITORING'],
  PARTIAL_FILL: ['MONITORING'],  // Accept partial, cancel remainder
  MONITORING: ['EXITED', 'FAILED'],
  // Terminal states: EXITED, TIMED_OUT, CANCELLED, FAILED
};
```

**Timeout logic:** Each order gets a `submittedAtMs` timestamp. On every tick, check if `Date.now() - submittedAtMs > 30_000`. If so, attempt cancel via CLOB API, transition to TIMED_OUT.

### Pattern 2: Two-Layer Retry
**What:** Per-order retries (micro) + circuit breaker (macro)
**Why two layers:** Per-order retries handle transient failures on individual orders. Circuit breaker prevents cascading failures when the CLOB API is down.

```javascript
// Layer 1: Per-order retry (in LiveExecutor)
async function withRetry(fn, { maxAttempts: 3, delays: [1000, 2000, 4000] }) {
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      return await fn();
    } catch (err) {
      if (!isRetryable(err) || attempt === maxAttempts) throw err;
      await sleep(delays[attempt - 1]);
    }
  }
}

// Layer 2: Circuit breaker (existing in polymarket.js)
// 3 consecutive failures -> open for 5s -> 10s -> 20s -> 60s cap
```

**Error classification:**
- Retryable: `err.code === 'ECONNRESET'`, `err.code === 'ETIMEDOUT'`, `status >= 500`, `status === 429`
- Fatal: `status === 401`, `status === 403`, `status === 400`, `status === 422`

### Pattern 3: Position Reconciliation
**What:** Compare local tracking state with CLOB API state every tick
**Algorithm:**
1. Fetch CLOB positions/orders via existing `client.getTrades()` and `client.getOpenOrders()` (if available)
2. Compute local positions from OrderManager tracked orders
3. Compare: token IDs, quantities, sides
4. If mismatch: log discrepancy details, set dashboard indicator to RED, emit structured event for Phase 4 webhooks
5. Never auto-correct

### Pattern 4: Fee-Aware Sizing
**What:** Subtract estimated fees from trade size before order submission
**Implementation:** FeeService.getFeeRateBps() already returns fee rate. Compute: `effectiveSize = sizeUsd * (1 - feeRateBps/10000)`. This ensures the actual position after fees matches the intended risk amount.

### Anti-Patterns to Avoid
- **Auto-correcting reconciliation mismatches** — User explicitly wants log + alert only. Auto-correction could compound errors.
- **Replacing the circuit breaker** — The existing module-level circuit breaker in polymarket.js must be preserved. Per-order retries are an additional layer below it.
- **Blocking the main loop for retries** — Retries happen within the order execution path, not the main 1s tick loop. If retries take too long, the next tick proceeds normally.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| State machine library | Complex FSM framework | Simple object-based transitions | Only 8 states, simple transitions; a library is overkill |
| Retry library | Generic retry framework | Simple async retry wrapper | 3 attempts with fixed delays; under 20 lines of code |
| Reconciliation engine | Complex diff algorithm | Direct field comparison | Comparing token IDs + quantities; not complex enough for a library |

**Key insight:** This phase is about hardening existing infrastructure, not building new abstractions. Keep implementations simple and directly in the existing class hierarchy.

## Common Pitfalls

### Pitfall 1: Race Conditions in Order Lifecycle
**What goes wrong:** Multiple ticks can process the same order simultaneously, causing duplicate cancel attempts or state transitions.
**Why it happens:** The main loop runs every 1s, and CLOB API calls are async. Two ticks might see the same order as pending.
**How to avoid:** Use a per-order lock flag (`_processingCancel`) or track last-attempted-cancel timestamp to prevent duplicate cancellation.
**Warning signs:** Duplicate CLOB cancel API calls, "order already cancelled" errors in logs.

### Pitfall 2: Partial Fill Complexity
**What goes wrong:** Accepting partial fills means the position size doesn't match what was planned. Exit sizing must use actual fill, not requested size.
**Why it happens:** CLOB orderbooks have limited depth; large orders may only partially fill.
**How to avoid:** After accepting partial fill, update OrderManager with actual fill size. Exit orders must reference the actual position size, not the originally requested size.
**Warning signs:** Exit orders for more shares than actually held.

### Pitfall 3: Kill-Switch Override Loop
**What goes wrong:** User overrides kill-switch, losses continue, kill-switch triggers again, user overrides again — infinite loss loop.
**Why it happens:** Override removes the blocker but doesn't address underlying market conditions.
**How to avoid:** Log each override with timestamp. The kill-switch can re-trigger after override (user confirmed this behavior). Consider displaying "Overridden N times today" in dashboard.
**Warning signs:** Multiple overrides in a single trading day.

### Pitfall 4: Reconciliation Noise
**What goes wrong:** Minor timing differences between local state and CLOB API cause false discrepancy alerts.
**Why it happens:** CLOB API may have slight propagation delays; a just-placed order might not appear in the next API call.
**How to avoid:** Use a grace window (e.g., 5-10 seconds after order submission) before flagging discrepancies. Compare only settled/confirmed state, not in-flight orders.
**Warning signs:** Frequent yellow->red->green flicker in dashboard sync indicator.

### Pitfall 5: Fee Estimation Accuracy
**What goes wrong:** Estimated fees differ from actual fees, causing position size mismatch.
**Why it happens:** Fee rates can change between estimation and execution; Polymarket may have complex fee tiers.
**How to avoid:** Use FeeService cached rate (already has 30s TTL). Accept that estimation is approximate — the goal is "close enough" not "exact". Log actual vs estimated for monitoring.
**Warning signs:** Consistent over/under-sizing by the fee amount.

## Code Examples

### Order Lifecycle State Machine
```javascript
class OrderLifecycle {
  constructor(orderId, meta) {
    this.orderId = orderId;
    this.state = 'SUBMITTED';
    this.meta = meta;
    this.timestamps = { SUBMITTED: Date.now() };
    this.fillSize = 0;
    this.fillPrice = 0;
  }

  transition(newState) {
    if (!TRANSITIONS[this.state]?.includes(newState)) {
      console.warn(`Invalid transition: ${this.state} -> ${newState}`);
      return false;
    }
    this.state = newState;
    this.timestamps[newState] = Date.now();
    return true;
  }

  isTimedOut(timeoutMs = 30_000) {
    if (this.state === 'SUBMITTED' || this.state === 'PENDING') {
      return Date.now() - this.timestamps.SUBMITTED > timeoutMs;
    }
    return false;
  }

  isTerminal() {
    return ['EXITED', 'TIMED_OUT', 'CANCELLED', 'FAILED'].includes(this.state);
  }
}
```

### Retry Wrapper with Error Classification
```javascript
function isRetryableError(err) {
  if (err?.code === 'ECONNRESET' || err?.code === 'ETIMEDOUT' || err?.code === 'ENOTFOUND') return true;
  const status = err?.response?.status ?? err?.status;
  if (status >= 500) return true;
  if (status === 429) return true;
  return false;
}

async function withOrderRetry(fn, opts = {}) {
  const maxAttempts = opts.maxAttempts ?? 3;
  const delays = opts.delays ?? [1000, 2000, 4000];

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      return await fn();
    } catch (err) {
      if (!isRetryableError(err) || attempt === maxAttempts) {
        // Emit structured failure event
        return { failed: true, error: err, retryCount: attempt, orderId: opts.orderId };
      }
      const delay = Math.min(delays[attempt - 1] ?? 4000, 30000);
      await new Promise(r => setTimeout(r, delay));
    }
  }
}
```

### Structured Failure Event
```javascript
function createFailureEvent(orderId, error, retryCount) {
  return {
    type: 'ORDER_FAILED',
    orderId,
    error: {
      message: error?.message || String(error),
      code: error?.code || null,
      status: error?.response?.status || null,
      retryable: isRetryableError(error),
    },
    retryCount,
    timestamp: new Date().toISOString(),
    // Phase 4 webhook consumption fields
    severity: 'critical',
    category: 'order_execution',
  };
}
```

## State of the Art

| Old Approach | Current Approach | Impact |
|--------------|------------------|--------|
| Fire-and-forget orders | Full lifecycle tracking | Can detect stuck/failed orders |
| No reconciliation | Per-tick reconciliation | Catches state drift early |
| Single retry layer (circuit breaker) | Two-layer (per-order + circuit breaker) | Handles transient errors without tripping circuit breaker |
| Fee as observability only | Fee-aware sizing | Actual position matches intended risk |
| No daily limit enforcement | Kill-switch with override | Prevents catastrophic daily losses |

## Open Questions

1. **CLOB API getOpenOrders() availability**
   - What we know: `client.getTrades()` exists and is used. `client.getOpenOrders()` may or may not be available on the CLOB client.
   - What's unclear: The exact method to fetch open/pending orders from the Polymarket CLOB API for reconciliation.
   - Recommendation: Check the @polymarket/clob-client API at runtime. If getOpenOrders exists, use it. Otherwise, reconcile using trade history only and flag limitation.

2. **Cancel order API**
   - What we know: The CLOB client can create orders. Cancellation may be via `client.cancelOrder(orderId)` or similar.
   - What's unclear: Exact cancellation API and its behavior for partially filled orders.
   - Recommendation: Attempt `client.cancelOrder()` or `client.cancelAll()`. Handle gracefully if not available — log warning and transition to FAILED state.

3. **Fee rate source for estimation**
   - What we know: FeeService.getFeeRateBps() exists with caching.
   - What's unclear: Whether the current fee source is accurate enough for pre-trade sizing.
   - Recommendation: Use existing FeeService. Default to a conservative estimate (e.g., 200 bps) if lookup fails. This is estimation, not exact calculation.

## Sources

### Primary (HIGH confidence)
- Codebase analysis: LiveExecutor.js, OrderManager.js, FeeService.js, TradingState.js, entryGate.js, sizing.js, polymarket.js, config.js
- CONTEXT.md user decisions (locked decisions)

### Secondary (MEDIUM confidence)
- Polymarket CLOB client API — inferred from existing usage patterns in LiveExecutor.js

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - All existing codebase modules fully analyzed
- Architecture: HIGH - Patterns extend existing proven architecture
- Pitfalls: HIGH - Based on actual codebase race conditions and API patterns observed

**Research date:** 2026-02-23
**Valid until:** 2026-03-23 (stable internal codebase)

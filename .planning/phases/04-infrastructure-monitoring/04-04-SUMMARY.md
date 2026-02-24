# Plan 04-04 Summary: Zero-Downtime Deployment (INFRA-08)

**Status:** Complete
**Requirement:** INFRA-08 — Zero-downtime deployment with instance coordination
**Wave:** 2

## What Was Built

### Task 1: File-based trading lock with heartbeat

**Files created:**
- `src/infrastructure/deployment/tradingLock.js`
- `test/infrastructure/tradingLock.test.js`

**Changes:**
1. Created `TradingLock` class:
   - File-based lock (`trading.lock`) with instance ID, PID, heartbeat timestamp
   - Configurable stale threshold (default 30s) and heartbeat interval (default 10s)
   - Random hex instance ID for each process

2. Lock operations:
   - `acquireLock()` — attempt to acquire:
     - No lock file: acquire immediately (reason: `new_lock`)
     - Stale heartbeat (>30s): take over (reason: `takeover_stale`)
     - Same instance: already held (reason: `already_held`)
     - Other active instance: fail (reason: `held_by_other`, holderId returned)
   - `releaseLock()` — remove lock file (only if held by this instance)
   - `isLockHolder()` — verify from file (catches takeover by another process)
   - `waitForLock(timeoutMs, pollIntervalMs)` — async polling with timeout

3. Heartbeat:
   - `updateHeartbeat()` — refreshes timestamp in lock file
   - `_startHeartbeat()` — interval timer (unref'd so it doesn't prevent exit)
   - `stopHeartbeat()` — clears interval

4. Diagnostics:
   - `getStatus()` — returns isHolder, instanceId, lockExists, lockHolder, heartbeatAge, staleThresholdMs, isStale

5. Singleton pattern:
   - `getTradingLock(opts)` / `resetTradingLock()` for singleton management

6. Tests: 12 test cases covering:
   - Acquire when free, stale, held by other, same instance
   - Release removes file, release doesn't delete other's lock
   - isLockHolder before/after acquire/release
   - Heartbeat timestamp refresh
   - Status reporting
   - waitForLock (immediate success, stale expiry)
   - Two-instance coordination sequence

### Task 2: Graceful shutdown handler

**Files created:**
- `src/infrastructure/deployment/gracefulShutdown.js`

**Changes:**
1. Created `installGracefulShutdown(opts)` function:
   - Registers handlers for SIGTERM and SIGINT
   - Idempotent (ignores duplicate signals)

2. Drain sequence (on SIGTERM/SIGINT):
   1. Stop accepting new trades (engine.tradingEnabled = false)
   2. Drain open positions — poll executor for position close (up to 5 min timeout)
   3. Persist critical state via StateManager
   4. Release trading lock
   5. Close trade store (SQLite)
   6. Close HTTP server
   7. Send webhook notification (GRACEFUL_SHUTDOWN)
   8. Exit cleanly (process.exit(0))

3. Crash handler:
   - `uncaughtException` — persist state + send crash webhook + exit(1)
   - `unhandledRejection` — log but don't exit (prevent cascade)

4. Testing support:
   - `onDrainComplete` callback (skips process.exit in tests)
   - Returns `{ isShuttingDown }` function for status queries

### Task 3: Integration into main entry point

**Files modified:**
- `src/index.js`

**Changes:**
1. Phase 4 initialization block in `startApp()`:
   - StateManager startup with crash detection and logging
   - WebhookService initialization (crash alert if crash detected)
   - TradingLock acquisition with 35s wait timeout
   - State restoration into engine.state on crash recovery
   - Graceful shutdown handler installation with all dependencies

2. Main loop enhancements:
   - Periodic state persistence (every ~30 ticks via stateManager.persistState)
   - Webhook alert checks for kill-switch activation, circuit breaker trip, and ORDER_FAILED events
   - Failure events consumed from LiveExecutor.getFailureEvents()

3. HTTP server capture:
   - `startUIServer()` return value stored as `_httpServer` for graceful shutdown

4. Phase 4 status logging on startup:
   - Logs state manager, webhook service, and trading lock status

## Verification

### Automated (run manually)
```bash
cd /Users/elijahprince/Dev/polymarket-btc-5m-assistant
node --test test/infrastructure/tradingLock.test.js
```

### Manual
- Start server with `npm start` — verify `trading.lock` file appears in data directory
- Check lock file contains valid JSON with instanceId, pid, heartbeat, acquiredAt
- Start a second instance — should log "Lock held by another instance" and wait/fail
- Stop first instance (Ctrl+C) — verify graceful drain sequence in console
- Second instance should acquire lock after first releases
- Kill process with `kill -9` — lock becomes stale after 30s, new instance takes over
- GET /health returns lock status and persistence info

## Requirements Satisfied

- **INFRA-08**: Zero-downtime deployment with instance coordination
  - File-based trading lock prevents duplicate execution across instances
  - Heartbeat (10s interval, 30s stale threshold) detects dead instances
  - Graceful drain on SIGTERM waits for open position close (5 min timeout)
  - State persisted before exit, restored on next startup
  - Lock released on clean shutdown, stale-detected on crash
  - Enhanced /health endpoint for platform readiness checks
  - Webhook notification on shutdown and crash

## Git Commands (manual execution)

```bash
git add src/infrastructure/deployment/tradingLock.js src/infrastructure/deployment/gracefulShutdown.js test/infrastructure/tradingLock.test.js src/index.js
git commit -m "feat: zero-downtime deployment with trading lock and graceful drain (INFRA-08)

- Create TradingLock with file-based instance coordination and heartbeat
- 30s stale threshold enables dead instance takeover
- Graceful shutdown: drain positions, persist state, release lock, close DB
- SIGTERM/SIGINT handlers with 5-minute drain timeout for 5m contracts
- Integrate all Phase 4 modules into main entry point
- Periodic state persistence and webhook alerts in main loop

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

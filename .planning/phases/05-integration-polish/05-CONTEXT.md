# Phase 5: Integration & Polish - Context

**Gathered:** 2026-02-23
**Status:** Ready for planning

<domain>
## Phase Boundary

End-to-end validation across all 4 prior phases, documentation updates, dashboard integration polish, and production deployment readiness. This phase ensures everything works together, is well-documented, and can be confidently deployed.

Requirements covered: Cross-cutting (validates ANLYT-01..04, PROF-01..04, LIVE-01..05, INFRA-05..08 work together).

What this phase does NOT include: new trading strategies, new indicators, new analytics features, mobile app, multi-user support.

</domain>

<decisions>
## Implementation Decisions

### End-to-End Validation Approach
- Automated integration tests in Node.js — spin up engine programmatically, run through each path, assert outcomes. Runnable via `npm test`. Repeatable and CI-friendly.
- Mock CLOB with test doubles for live trading validation — mock LiveExecutor simulates order lifecycle (submit -> fill -> exit) without hitting real APIs. Validates full flow without real money risk.
- Process.kill() for crash recovery simulation — integration test starts engine, simulates a trade, kills process (SIGKILL), restarts, and verifies state was recovered via PID lock + state file path.
- All tests pass in single `npm test` — both existing unit tests and new integration tests run together. Any failure = not ready for deploy.

### Documentation Scope & Format
- Full documentation suite — update CLAUDE.md, create DEPLOYMENT.md, rewrite README.md, update CHANGELOG.md
- Operational runbook in DEPLOYMENT.md — step-by-step: env var reference, DigitalOcean App Platform config, health check setup, webhook configuration, monitoring setup, common troubleshooting
- ASCII diagrams in CLAUDE.md — extend existing architecture diagram with Phase 1-4 additions. No external tooling needed.
- Comprehensive changelog — document all 4 phases as a release (v1.0) in CHANGELOG.md, grouped by phase with key features listed
- DigitalOcean-specific section in DEPLOYMENT.md — App Spec YAML example, health check config, env var setup, volume mount for SQLite/state files, deploy hook for graceful shutdown

### Dashboard Integration Polish
- Graceful degradation with status indicators — show all UI elements but gray out / show "Not Configured" for features missing setup. User can see what's available.
- Compact status bar with 6 essential indicators — Mode (Paper/Live), Trading (Active/Stopped), Kill-switch (OK/Triggered), Sync (green/yellow/red dot), SQLite (Connected/Fallback), Uptime. One row at top.
- Shared data source for consistency — all tabs read from the same SQLite store via the same API endpoints. Consistency guaranteed by architecture.
- Keep current 3 tabs — Dashboard, Analytics, Optimizer. Phase 3-4 features integrate into Dashboard tab (kill-switch, lifecycle, health). No new tabs.
- SQLite auto-fallback to JSON with info banner — dashboard works normally using JSON ledger if better-sqlite3 not installed. Subtle banner nudges user to install.
- Webhook status indicator only — "Webhooks: Configured (Slack)" or "Webhooks: Not configured" in status bar. No test button or delivery log.
- Basic mobile responsive check — verify dashboard doesn't break on mobile viewport, fix major layout issues. Desktop is primary platform.

### Production Deploy Checklist
- Automated pre-flight script (`npm run preflight`) — checks: all tests pass, required env vars set, SQLite DB accessible, config values sane, webhook URL reachable if configured. Pass/fail output.
- Startup env var validation — validate required env vars on startup (POLYMARKET_SLUG, price feed config). Warn but continue for optional ones (WEBHOOK_URL, DAILY_LOSS_LIMIT). Clear log messages.
- NODE_ENV-based production defaults — when NODE_ENV=production: stricter logging (no debug), require kill-switch config, default shorter poll intervals, enable SQLite auto-migration. Standard Node.js convention.

### Claude's Discretion
- Integration test file organization and naming
- Exact status bar styling and layout
- README.md structure and sections
- CHANGELOG.md format (Keep-a-Changelog vs custom)
- Pre-flight script output format and error messages
- Which env vars are "required" vs "optional" classification
- Specific NODE_ENV=production behavior changes beyond those listed
- Dashboard degradation indicator styling (gray text, dotted borders, etc.)
- Test double implementation for mock LiveExecutor

</decisions>

<specifics>
## Specific Ideas

- The existing test runner uses Node.js native `test` runner (`node:test` + `node:assert`) — new integration tests should follow the same pattern
- Phase 3 already has extensive unit tests (59 tests across 4 files) — integration tests complement, not duplicate, these
- The existing CLAUDE.md architecture section has ASCII diagrams that should be extended, not replaced
- The Phase 4 `getTradeStore()` with fallback pattern already exists — dashboard degradation should follow the same fallback approach
- DigitalOcean App Platform uses `--run-command` for startup and supports health check paths — map to existing `/health` endpoint

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 05-integration-polish*
*Context gathered: 2026-02-23*

# Plan 05-04: Production Readiness

**Status:** Complete
**Executed:** 2026-02-23

## Goal

Create automated pre-flight script, startup environment validation, and NODE_ENV-based production defaults.

## Files Created/Modified

| File | Action | Purpose |
|------|--------|---------|
| `scripts/preflight.js` | Created | Pre-flight production readiness check: runs tests, validates env vars, checks SQLite, validates config sanity, tests webhook reachability |
| `src/infrastructure/deployment/envValidation.js` | Created | Startup env var validation with clear log messages, production-specific checks, config sanity checks |
| `src/index.js` | Modified | Added `logEnvValidation()` call at startup |
| `package.json` | Modified | Added `"preflight": "node scripts/preflight.js"` script |

## Key Decisions

- Pre-flight script exits with code 1 on failures, 0 on success (CI-friendly)
- Required env vars produce warnings (bot continues), not hard errors
- Production mode (NODE_ENV=production): warnings logged as errors, kill-switch config required
- Webhook reachability tested via HEAD request with 5s timeout (accepts 405/400 as "reachable")
- Config sanity checks: STAKE_PCT range, trade size vs bounds, daily loss vs balance
- Pre-flight reads .env file for checking (mimics dotenv behavior)

## Verification

Run: `npm run preflight` — should show pass/fail/warn for each check.

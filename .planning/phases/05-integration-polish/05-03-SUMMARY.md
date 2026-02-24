# Plan 05-03: Dashboard Polish

**Status:** Complete
**Executed:** 2026-02-23

## Goal

Polish the dashboard with a compact status bar, graceful degradation for unconfigured features, SQLite fallback banner, and basic mobile responsive fixes.

## Files Modified

| File | Changes |
|------|---------|
| `src/ui/index.html` | Added compact 6-indicator status bar (Mode, Trading, Kill-Switch, SQLite, Webhooks, Uptime), SQLite fallback banner |
| `src/ui/style.css` | Status bar styles, fallback banner styles, degraded indicator styles, improved mobile breakpoints (tables scroll, status bar wraps, tabs scroll) |
| `src/ui/script.js` | Status bar population from /api/metrics, kill-switch status bar updates from /api/status, metrics polling (10s interval) |

## Key Decisions

- Status bar fetches from `/api/metrics` every 10 seconds (separate from main 1.5s poll)
- Kill-switch indicator also updates from the main status poll for faster response
- SQLite fallback banner is hidden by default, shown only when `/api/metrics` reports `persistence.sqlite === false`
- Mobile: tables use `overflow-x: auto` for horizontal scrolling rather than hiding columns
- Status bar wraps to two rows on very small screens (< 500px) instead of horizontal scroll

## Verification

Open dashboard in browser. Status bar should show 6 indicators. Resize to mobile width. Tables should scroll horizontally. SQLite status should show "Connected" or "Fallback (JSON)" based on installation.

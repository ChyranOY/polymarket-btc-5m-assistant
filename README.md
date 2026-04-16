# Polymarket BTC 5m Assistant (Rust)

Small, fast Rust bot for Polymarket's 5-minute BTC Up/Down contracts. Paper-mode first,
live-mode planned. All trades land in the shared dashboard Supabase so existing analytics
keep working.

## Status

Built (working):
- Paper executor, event-driven market rollover, entry/exit pure-fns, HTTP API, static web UI.
- CLOB WS book feed — primary price source; REST `/price` only fires as fallback.
- `cargo test` — 39 unit tests + 2 integration tests pass.

Not yet built (follow-ups):
- Live executor (EIP-712 order signing via `alloy`) — scaffolded but not implemented.
- `signal_ticks` batched writer.

## Quick start (paper mode)

```bash
cp .env.example .env
# Edit .env: SUPABASE_URL + SUPABASE_SERVICE_ROLE_KEY (optional), CONTROL_TOKEN (optional)
cargo run --release
# Open http://localhost:3000/ in a browser
```

By default `TRADING_MODE=paper` and `TRADING_ENABLED_ON_BOOT=false`, so the bot boots
idle. Paste the CONTROL_TOKEN (if set) in the UI and click Start.

## Deploy to Digital Ocean App Platform

Push-to-deploy via the committed `.do/app.yaml` and `Dockerfile`.

1. **Fork / push this repo to GitHub.**
2. Edit `.do/app.yaml` and replace `<github-owner>/polymarket-btc-5m-assistant` with
   the real repo slug.
3. Install `doctl` and authenticate (`doctl auth init`). First deploy:
   ```bash
   doctl apps create --spec .do/app.yaml
   ```
   …or paste the YAML into the DO web console under *Apps → Create App → Import App Spec*.
4. In the console → **Settings → App-Level Environment Variables**, paste values for
   every `type: SECRET` key:
   - `SUPABASE_URL`, `SUPABASE_SERVICE_ROLE_KEY`, `CONTROL_TOKEN` (required)
   - `PRIVATE_KEY`, `CLOB_API_KEY`, `CLOB_SECRET`, `CLOB_PASSPHRASE`, `FUNDER_ADDRESS`
     (leave empty while running paper)
5. Wait for the first build (~4–6 min — Rust compile dominates). Tail logs:
   ```bash
   doctl apps logs <app-id> --follow
   ```
   Look for `polymarket-btc-5m boot ... supabase_ready=true`, `clob_ws: connected`,
   `market: initial slug=btc-updown-5m-...`.
6. Open the app URL from the console → dashboard loads → paste your `CONTROL_TOKEN`
   → click **Start**. A subsequent `git push` redeploys automatically.

**Important deployment behaviors**
- **Single instance**: `instance_count: 1`. The bot is stateful (one open position
  at a time). Never scale horizontally.
- **Ephemeral storage**: `paper_ledger.json` is lost on each redeploy. That's fine —
  `boot_reconcile` in `src/main.rs` hydrates `daily_pnl` from Supabase on every boot
  and marks any abandoned `OPEN` trade as `exitReason=abandoned_by_restart`.
- **SIGTERM graceful close**: On every redeploy DO sends `SIGTERM`. The bot flips
  `trading_enabled=false`, tries to close an open position (20s budget), and patches
  the Supabase row to `CLOSED` with `exitReason=shutdown`. Then exits.
- **Port**: `$PORT` (set by DO to match `http_port`) overrides `HTTP_PORT`. Both work locally.

## Project layout

```
Cargo.toml
rust-toolchain.toml
src/
  main.rs                         # tokio runtime, wiring
  lib.rs                          # re-exports modules for integration tests
  config.rs                       # env parsing
  error.rs                        # thiserror enum
  time_utils.rs                   # PST hours / weekend check
  model.rs                        # Trade, OpenPosition, MarketSnapshot, Side, Mode
  data/
    gamma.rs                      # Gamma /events?series_id client
    clob_rest.rs                  # CLOB /price and /book
  engine/
    entry.rs / exit.rs / sizing.rs  # pure fns (heavily tested)
    state.rs                      # EngineState + circuit breaker
    tick.rs                       # 1s tick loop orchestration
  exec/
    mod.rs                        # Executor trait, Open/Close req/result
    paper.rs                      # PaperExecutor (fee + slippage + ledger)
  store/
    supabase.rs                   # PostgREST client for trades + signal_ticks
  market/
    scheduler.rs                  # event-driven rollover (tokio::sleep_until)
  api/
    routes.rs                     # axum: /health /status /trades /positions /trading/* /mode
static/
  index.html / app.js / style.css # minimal dashboard (~300 lines total)
tests/
  paper_flow.rs                   # end-to-end paper open/close
```

## Entry gate (7 `SkipReason`s)

```
TradingDisabled | OpenPositionExists | OutsideTradingHours | MarketNotAlive
| CheapSideOutOfRange | PricesUnavailable | CircuitBreakerTripped
```

v1 edge: buy the cheap side (ask ∈ [0.15, 0.45]) during PST 06:00–17:00 weekdays,
≥ 1.5 min to settlement.

## Exit gate (4 `ExitReason`s)

```
StopLoss (pnl ≤ -30% of contract_size)
| SettlementImminent (< 60s left)
| MarketRolled (new slug AND past old end_date)
| ManualKillSwitch
```

## HTTP API

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET    | `/health`          | — | `{ ok, version, uptime_s }` |
| GET    | `/status`          | — | engine + market + balance + last skip |
| GET    | `/trades?limit=20` | — | Supabase pass-through; falls back to in-memory cache |
| GET    | `/positions`       | — | current open position (≤1 in v1) |
| POST   | `/trading/start`   | Bearer `CONTROL_TOKEN` | enable trading |
| POST   | `/trading/stop`    | Bearer `CONTROL_TOKEN` | disable trading |
| POST   | `/mode`            | Bearer `CONTROL_TOKEN` | `{ "mode": "paper" \| "live" }` |
| GET    | `/ui/*`            | — | static dashboard |

If `CONTROL_TOKEN` is empty, all POSTs are unauthenticated — OK for localhost, not for
a public deployment.

## Known environmental note (2026-04)

Polymarket's `btc-up-or-down-5m` series had no active events when this repo was built
(`curl gamma-api.polymarket.com/events?series_id=10684&active=true&closed=false` → empty).
The bot handles this cleanly: the scheduler retries every 2s, and `/status.market` stays
`null`. When the series comes back online (or is migrated to a new `series_id`), set
`POLYMARKET_SERIES_ID` in `.env`.

## Tests

```bash
cargo test           # 37 tests total (35 unit + 2 integration)
cargo check          # fast type-check
cargo build --release
```

## License

Private. Not open source.

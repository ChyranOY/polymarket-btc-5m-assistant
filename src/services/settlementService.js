const SETTLEMENT_SLUG_RE = /(\d{10})$/;
const CHECK_INTERVAL_MS = 30_000;
const SETTLEMENT_GRACE_MS = 5_000;
const MAX_BACKFILL_AGE_MS = 10 * 60_000;

let _lastCheckAtMs = 0;

function toFiniteNumber(value) {
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

export function parseSettlementMsFromSlug(marketSlug) {
  const match = typeof marketSlug === 'string' ? marketSlug.match(SETTLEMENT_SLUG_RE) : null;
  if (!match) return null;
  const tsSec = Number(match[1]);
  if (!Number.isFinite(tsSec)) return null;
  return tsSec * 1000;
}

export function deriveMarketSettlementTime(marketSlug) {
  const settlementMs = parseSettlementMsFromSlug(marketSlug);
  return settlementMs ? new Date(settlementMs).toISOString() : null;
}

function hasSettlementBackfill(trade) {
  return trade?.settlementSide !== undefined && trade?.settlementSide !== null;
}

async function persistTrade(store, trade) {
  if (!store || !trade?.id) return false;
  if (typeof store.insertTrade === 'function') {
    await store.insertTrade(trade, trade.mode || 'paper');
    return true;
  }
  if (typeof store.upsertTrade === 'function') {
    await store.upsertTrade(trade);
    return true;
  }
  return false;
}

export async function checkSettlements({ currentPrice, nowMs = Date.now() } = {}) {
  if (nowMs - _lastCheckAtMs < CHECK_INTERVAL_MS) return;
  _lastCheckAtMs = nowMs;

  const btcNow = toFiniteNumber(currentPrice);
  if (btcNow === null) return;

  const getStore = globalThis.__tradeStore_getTradeStore;
  if (typeof getStore !== 'function') return;

  let store;
  try {
    store = getStore();
  } catch {
    return;
  }
  if (!store || typeof store.getAllTrades !== 'function') return;

  const trades = await store.getAllTrades();
  if (!Array.isArray(trades) || trades.length === 0) return;

  for (const trade of trades) {
    if (trade?.status !== 'CLOSED' || hasSettlementBackfill(trade)) continue;

    const settlementMs = parseSettlementMsFromSlug(trade.marketSlug);
    if (!settlementMs) continue;
    if (nowMs < settlementMs + SETTLEMENT_GRACE_MS) continue;
    if (nowMs > settlementMs + MAX_BACKFILL_AGE_MS) continue;

    const btcAtEntry = toFiniteNumber(trade.btcSpotAtEntry);
    if (btcAtEntry === null) continue;

    const settlementSide = btcNow > btcAtEntry ? 'UP' : 'DOWN';
    const updatedTrade = {
      ...trade,
      marketSettlementTime: trade.marketSettlementTime || new Date(settlementMs).toISOString(),
      btcAtSettlement: btcNow,
      settlementSide,
      directionCorrect: trade.side === settlementSide,
    };

    try {
      const saved = await persistTrade(store, updatedTrade);
      if (!saved && typeof globalThis.__syncTradeToStore === 'function') {
        await globalThis.__syncTradeToStore(updatedTrade, updatedTrade.mode || 'paper');
      }
      console.log(
        `[Settlement] Trade ${updatedTrade.id}: settled ${settlementSide}, ` +
        `picked ${updatedTrade.side} -> ${updatedTrade.directionCorrect ? 'CORRECT' : 'WRONG'}`
      );
    } catch {
      // Settlement enrichment is best-effort; never break the main loop.
    }
  }
}

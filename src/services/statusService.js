import crypto from 'crypto';
import { CONFIG } from '../config.js';
import { initializeLedger, getLedger, recalculateSummary, getOpenTrade } from '../paper_trading/ledger.js';
import { fetchCollateralBalance } from '../live_trading/clob.js';
import { getLiveLedger } from '../live_trading/ledger.js';
import { getPacificTimeInfo } from '../domain/entryGate.js';

// Diagnostic: unique ID per process instance + boot timestamp.
// If the UI sees different instanceIds across consecutive polls, there are
// multiple instances or the app is crash-restarting.
const _instanceId = crypto.randomBytes(4).toString('hex');
const _bootedAtMs = Date.now();

export async function assembleStatus() {
  await initializeLedger();

  // New unified engine + mode manager (exposed by index.js)
  const engine = globalThis.__tradingEngine ?? null;
  const modeManager = globalThis.__modeManager ?? null;

  const ledgerData = getLedger();
  const openTrade = getOpenTrade();

  // Entry debug from unified engine
  const entryDebug = engine?.lastEntryStatus ?? null;
  const blockerSummary = engine?.state?.getBlockerSummary?.(10) ?? null;

  const summary = ledgerData.summary ?? recalculateSummary(ledgerData.trades ?? []);

  const starting = CONFIG.paperTrading.startingBalance ?? 1000;
  const baseRealized = typeof summary.totalPnL === 'number' ? summary.totalPnL : 0;
  const offset = (ledgerData.meta && typeof ledgerData.meta.realizedOffset === 'number' && Number.isFinite(ledgerData.meta.realizedOffset))
    ? ledgerData.meta.realizedOffset
    : 0;
  const realized = baseRealized + offset;
  const balance = starting + realized;

  let liveCollateral = null;
  if (CONFIG.liveTrading?.enabled) {
    try {
      liveCollateral = await fetchCollateralBalance();
    } catch (e) {
      liveCollateral = { error: e?.message || String(e) };
    }
  }

  const liveLedger = CONFIG.liveTrading?.enabled ? (getLiveLedger()?.trades ?? []) : [];

  const dailyPnl = engine?.state?.todayRealizedPnl ?? null;

  return {
    status: { ok: true, updatedAt: new Date().toISOString(), _instanceId, _uptimeS: Math.round((Date.now() - _bootedAtMs) / 1000) },
    mode: modeManager?.getMode()?.toUpperCase() ?? (CONFIG.liveTrading?.enabled ? 'LIVE' : 'PAPER'),
    tradingEnabled: engine?.tradingEnabled ?? false,
    openTrade,
    entryDebug,
    blockerSummary,
    ledgerSummary: summary,
    balance: { starting, realized, balance },
    paperTrading: {
      enabled: CONFIG.paperTrading.enabled,
      stakePct: CONFIG.paperTrading.stakePct,
      minTradeUsd: CONFIG.paperTrading.minTradeUsd,
      maxTradeUsd: CONFIG.paperTrading.maxTradeUsd,
      stopLossPct: CONFIG.paperTrading.stopLossPct,
      flipOnProbabilityFlip: CONFIG.paperTrading.flipOnProbabilityFlip
    },
    liveTrading: {
      enabled: Boolean(CONFIG.liveTrading?.enabled),
      available: modeManager?.isLiveAvailable() ?? false,
      funder: process.env.FUNDER_ADDRESS || null,
      signatureType: process.env.SIGNATURE_TYPE || null,
      limits: CONFIG.liveTrading || null,
      collateral: liveCollateral,
      tradesCount: Array.isArray(liveLedger) ? liveLedger.length : 0,
      daily: {
        realizedPnlUsd: typeof dailyPnl === 'number' ? dailyPnl : null,
        maxDailyLossUsd: CONFIG.liveTrading?.maxDailyLossUsd ?? CONFIG.paperTrading.maxDailyLossUsd ?? null,
        remainingLossBudgetUsd: (typeof dailyPnl === 'number' && Number.isFinite(dailyPnl))
          ? (Number(CONFIG.liveTrading?.maxDailyLossUsd ?? CONFIG.paperTrading.maxDailyLossUsd ?? 0) + dailyPnl)
          : null
      },
      fees: engine?.executor?.feeService?.getSnapshot?.() ?? null,
      approvals: engine?.executor?.approvalService?.getStatus?.() ?? null,
    },
    entryThresholds: (() => {
      const { isWeekend, wd, hour } = getPacificTimeInfo();
      return {
      // Schedule context (Pacific time)
      isWeekend,
      pacificDay: wd,
      pacificHour: hour,
      weekendTighteningActive: isWeekend && Boolean(CONFIG.paperTrading.weekendTighteningEnabled ?? true),
      // Prob thresholds (MID includes midProbBoost)
      minProbEarly: CONFIG.paperTrading.minProbEarly ?? 0.52,
      minProbMid: (CONFIG.paperTrading.minProbMid ?? 0.53) + (CONFIG.paperTrading.midProbBoost ?? 0.01),
      minProbLate: CONFIG.paperTrading.minProbLate ?? 0.55,
      // Edge thresholds (MID includes midEdgeBoost)
      edgeEarly: CONFIG.paperTrading.edgeEarly ?? 0.02,
      edgeMid: (CONFIG.paperTrading.edgeMid ?? 0.03) + (CONFIG.paperTrading.midEdgeBoost ?? 0.01),
      edgeLate: CONFIG.paperTrading.edgeLate ?? 0.05,
      // Weekend tightening boosts
      weekendProbBoost: CONFIG.paperTrading.weekendProbBoost ?? 0.03,
      weekendEdgeBoost: CONFIG.paperTrading.weekendEdgeBoost ?? 0.03,
      // Market quality
      maxSpread: CONFIG.paperTrading.maxSpread ?? 0.012,
      weekendMaxSpread: CONFIG.paperTrading.weekendMaxSpread ?? 0.008,
      minLiquidity: CONFIG.paperTrading.minLiquidity ?? 500,
      weekendMinLiquidity: CONFIG.paperTrading.weekendMinLiquidity ?? 20000,
      minModelMaxProb: CONFIG.paperTrading.minModelMaxProb ?? 0.53,
      weekendMinModelMaxProb: CONFIG.paperTrading.weekendMinModelMaxProb ?? 0.6,
      // Filters
      minRangePct20: CONFIG.paperTrading.minRangePct20 ?? 0.0012,
      minBtcImpulsePct1m: CONFIG.paperTrading.minBtcImpulsePct1m ?? 0.0003,
      noTradeRsiMin: CONFIG.paperTrading.noTradeRsiMin ?? 30,
      noTradeRsiMax: CONFIG.paperTrading.noTradeRsiMax ?? 45,
      maxEntryPolyPrice: CONFIG.paperTrading.maxEntryPolyPrice ?? 0.65,
      // Guardrails
      circuitBreakerConsecutiveLosses: CONFIG.paperTrading.circuitBreakerConsecutiveLosses ?? 5,
      maxDailyLossUsd: CONFIG.paperTrading.maxDailyLossUsd ?? 50,
      lossCooldownSeconds: CONFIG.paperTrading.lossCooldownSeconds ?? 30,
      winCooldownSeconds: CONFIG.paperTrading.winCooldownSeconds ?? 30,
      noEntryFinalMinutes: CONFIG.paperTrading.noEntryFinalMinutes ?? 1.5,
      }; // end return
    })(),
    killSwitch: engine?.state?.getKillSwitchStatus?.(
      engine?.config?.maxDailyLossUsd ?? CONFIG.liveTrading?.maxDailyLossUsd ?? CONFIG.paperTrading?.maxDailyLossUsd ?? null,
    ) ?? null,
    orderLifecycle: engine?.executor?.orderManager?.getAllOrderViews?.() ?? [],
    reconciliation: engine?.executor?.getReconciliationStatus?.() ?? null,
    failureEvents: (engine?.executor?.getFailureEvents?.() ?? []).slice(-10),
    runtime: globalThis.__uiStatus ?? null
  };
}

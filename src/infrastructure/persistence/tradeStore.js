/**
 * @file SQLite trade store — structured persistence replacing JSON ledger reads.
 *
 * Uses better-sqlite3 for synchronous, fast, single-file database access.
 * Schema captures all enriched trade fields (20+ indicators) for SQL-based analytics.
 *
 * Architecture: Infrastructure layer (I/O).
 *
 * Usage:
 *   import { getTradeStore } from './tradeStore.js';
 *   const store = getTradeStore();       // singleton, auto-creates DB + tables
 *   store.insertTrade(trade);            // insert one trade
 *   store.getAllTrades();                 // all trades (replaces loadLedger())
 *   store.getClosedTrades();             // closed trades only
 *   store.getTradesByDateRange(from, to);
 *   store.getTradesByOutcome('win');
 */

import Database from 'better-sqlite3';
import path from 'node:path';
import fs from 'node:fs';

// ── Schema version ─────────────────────────────────────────────────
const SCHEMA_VERSION = 1;

// ── Default DB path ────────────────────────────────────────────────
const DEFAULT_DB_DIR = process.env.DATA_DIR || './data';
const DEFAULT_DB_PATH = path.join(DEFAULT_DB_DIR, 'trades.db');

// ── Singleton ──────────────────────────────────────────────────────
let _instance = null;

/**
 * Get the singleton TradeStore instance.
 * @param {Object} [opts]
 * @param {string} [opts.dbPath] - Override DB file path (for testing)
 * @returns {TradeStore}
 */
export function getTradeStore(opts = {}) {
  if (_instance && !opts.dbPath) return _instance;
  const store = new TradeStore(opts);
  if (!opts.dbPath) _instance = store;
  return store;
}

/**
 * Reset singleton (for testing).
 */
export function resetTradeStore() {
  if (_instance) {
    try { _instance.close(); } catch { /* ignore */ }
    _instance = null;
  }
}

// ── Trade store class ──────────────────────────────────────────────

export class TradeStore {
  /**
   * @param {Object} [opts]
   * @param {string} [opts.dbPath] - Path to SQLite file (default: ./data/trades.db)
   * @param {boolean} [opts.inMemory] - Use in-memory DB (for testing)
   */
  constructor(opts = {}) {
    const dbPath = opts.inMemory ? ':memory:' : (opts.dbPath || DEFAULT_DB_PATH);

    // Ensure directory exists
    if (dbPath !== ':memory:') {
      const dir = path.dirname(dbPath);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }
    }

    this.db = new Database(dbPath);

    // Enable WAL mode for better concurrent read performance
    this.db.pragma('journal_mode = WAL');
    this.db.pragma('synchronous = NORMAL');

    this._createSchema();
    this._prepareStatements();
  }

  // ─── Schema ────────────────────────────────────────────────────

  _createSchema() {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER PRIMARY KEY
      );

      CREATE TABLE IF NOT EXISTS trades (
        id TEXT PRIMARY KEY,
        timestamp TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'OPEN',
        side TEXT,
        entryPrice REAL,
        exitPrice REAL,
        shares REAL,
        contractSize REAL,
        pnl REAL DEFAULT 0,
        entryTime TEXT,
        exitTime TEXT,
        exitReason TEXT,
        entryPhase TEXT,
        marketSlug TEXT,
        sideInferred INTEGER,

        -- Indicator enrichment at entry
        modelProbAtEntry REAL,
        edgeAtEntry REAL,
        rsiAtEntry REAL,
        vwapDistAtEntry REAL,
        spreadAtEntry REAL,
        liquidityAtEntry REAL,
        volumeNumAtEntry REAL,
        btcSpotAtEntry REAL,
        spotImpulsePctAtEntry REAL,
        heikenColorAtEntry TEXT,
        heikenCountAtEntry INTEGER,
        rangePct20AtEntry REAL,
        recActionAtEntry TEXT,
        macdValueAtEntry REAL,
        macdHistAtEntry REAL,
        macdSignalAtEntry REAL,
        vwapSlopeAtEntry REAL,
        modelUpAtEntry REAL,
        modelDownAtEntry REAL,
        timeLeftMinAtEntry REAL,
        maxEntryPolyPriceAtEntry REAL,

        -- Exit enrichment
        btcSpotAtExit REAL,
        rsiAtExit REAL,
        macdHistAtExit REAL,
        vwapSlopeAtExit REAL,
        modelUpAtExit REAL,
        modelDownAtExit REAL,

        -- MFE/MAE
        maxUnrealizedPnl REAL,
        minUnrealizedPnl REAL,

        -- Entry gate snapshot (JSON string)
        entryGateSnapshot TEXT,

        -- Mode (paper/live)
        mode TEXT DEFAULT 'paper',

        -- Raw JSON blob for any extra fields not in schema
        extraJson TEXT,

        -- Timestamps
        createdAt TEXT DEFAULT (datetime('now')),
        updatedAt TEXT DEFAULT (datetime('now'))
      );

      -- Indexes for common query patterns
      CREATE INDEX IF NOT EXISTS idx_trades_status ON trades(status);
      CREATE INDEX IF NOT EXISTS idx_trades_timestamp ON trades(timestamp);
      CREATE INDEX IF NOT EXISTS idx_trades_exitTime ON trades(exitTime);
      CREATE INDEX IF NOT EXISTS idx_trades_side ON trades(side);
      CREATE INDEX IF NOT EXISTS idx_trades_mode ON trades(mode);
      CREATE INDEX IF NOT EXISTS idx_trades_entryPhase ON trades(entryPhase);
      CREATE INDEX IF NOT EXISTS idx_trades_marketSlug ON trades(marketSlug);

      -- Summary table (mirrors JSON ledger summary)
      CREATE TABLE IF NOT EXISTS trade_summary (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        totalTrades INTEGER DEFAULT 0,
        wins INTEGER DEFAULT 0,
        losses INTEGER DEFAULT 0,
        totalPnL REAL DEFAULT 0,
        winRate REAL DEFAULT 0,
        updatedAt TEXT DEFAULT (datetime('now'))
      );

      -- Meta table (mirrors JSON ledger meta)
      CREATE TABLE IF NOT EXISTS trade_meta (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        realizedOffset REAL DEFAULT 0,
        updatedAt TEXT DEFAULT (datetime('now'))
      );
    `);

    // Initialize summary/meta rows if not present
    const summaryRow = this.db.prepare('SELECT id FROM trade_summary WHERE id = 1').get();
    if (!summaryRow) {
      this.db.prepare('INSERT INTO trade_summary (id) VALUES (1)').run();
    }
    const metaRow = this.db.prepare('SELECT id FROM trade_meta WHERE id = 1').get();
    if (!metaRow) {
      this.db.prepare('INSERT INTO trade_meta (id) VALUES (1)').run();
    }

    // Track schema version
    const versionRow = this.db.prepare('SELECT version FROM schema_version LIMIT 1').get();
    if (!versionRow) {
      this.db.prepare('INSERT INTO schema_version (version) VALUES (?)').run(SCHEMA_VERSION);
    }
  }

  _prepareStatements() {
    this._insertStmt = this.db.prepare(`
      INSERT OR REPLACE INTO trades (
        id, timestamp, status, side, entryPrice, exitPrice, shares, contractSize,
        pnl, entryTime, exitTime, exitReason, entryPhase, marketSlug, sideInferred,
        modelProbAtEntry, edgeAtEntry, rsiAtEntry, vwapDistAtEntry, spreadAtEntry,
        liquidityAtEntry, volumeNumAtEntry, btcSpotAtEntry, spotImpulsePctAtEntry,
        heikenColorAtEntry, heikenCountAtEntry, rangePct20AtEntry, recActionAtEntry,
        macdValueAtEntry, macdHistAtEntry, macdSignalAtEntry, vwapSlopeAtEntry,
        modelUpAtEntry, modelDownAtEntry, timeLeftMinAtEntry, maxEntryPolyPriceAtEntry,
        btcSpotAtExit, rsiAtExit, macdHistAtExit, vwapSlopeAtExit,
        modelUpAtExit, modelDownAtExit,
        maxUnrealizedPnl, minUnrealizedPnl,
        entryGateSnapshot, mode, extraJson, updatedAt
      ) VALUES (
        @id, @timestamp, @status, @side, @entryPrice, @exitPrice, @shares, @contractSize,
        @pnl, @entryTime, @exitTime, @exitReason, @entryPhase, @marketSlug, @sideInferred,
        @modelProbAtEntry, @edgeAtEntry, @rsiAtEntry, @vwapDistAtEntry, @spreadAtEntry,
        @liquidityAtEntry, @volumeNumAtEntry, @btcSpotAtEntry, @spotImpulsePctAtEntry,
        @heikenColorAtEntry, @heikenCountAtEntry, @rangePct20AtEntry, @recActionAtEntry,
        @macdValueAtEntry, @macdHistAtEntry, @macdSignalAtEntry, @vwapSlopeAtEntry,
        @modelUpAtEntry, @modelDownAtEntry, @timeLeftMinAtEntry, @maxEntryPolyPriceAtEntry,
        @btcSpotAtExit, @rsiAtExit, @macdHistAtExit, @vwapSlopeAtExit,
        @modelUpAtExit, @modelDownAtExit,
        @maxUnrealizedPnl, @minUnrealizedPnl,
        @entryGateSnapshot, @mode, @extraJson, datetime('now')
      )
    `);

    this._getAllStmt = this.db.prepare('SELECT * FROM trades ORDER BY timestamp ASC');
    this._getClosedStmt = this.db.prepare("SELECT * FROM trades WHERE status = 'CLOSED' ORDER BY timestamp ASC");
    this._getOpenStmt = this.db.prepare("SELECT * FROM trades WHERE status = 'OPEN' ORDER BY timestamp ASC");
    this._getByIdStmt = this.db.prepare('SELECT * FROM trades WHERE id = ?');
    this._updateStmt = this.db.prepare(`
      UPDATE trades SET
        status = COALESCE(@status, status),
        exitPrice = COALESCE(@exitPrice, exitPrice),
        pnl = COALESCE(@pnl, pnl),
        exitTime = COALESCE(@exitTime, exitTime),
        exitReason = COALESCE(@exitReason, exitReason),
        maxUnrealizedPnl = COALESCE(@maxUnrealizedPnl, maxUnrealizedPnl),
        minUnrealizedPnl = COALESCE(@minUnrealizedPnl, minUnrealizedPnl),
        btcSpotAtExit = COALESCE(@btcSpotAtExit, btcSpotAtExit),
        rsiAtExit = COALESCE(@rsiAtExit, rsiAtExit),
        macdHistAtExit = COALESCE(@macdHistAtExit, macdHistAtExit),
        vwapSlopeAtExit = COALESCE(@vwapSlopeAtExit, vwapSlopeAtExit),
        modelUpAtExit = COALESCE(@modelUpAtExit, modelUpAtExit),
        modelDownAtExit = COALESCE(@modelDownAtExit, modelDownAtExit),
        updatedAt = datetime('now')
      WHERE id = @id
    `);

    this._deleteAllStmt = this.db.prepare('DELETE FROM trades');

    this._getSummaryStmt = this.db.prepare('SELECT * FROM trade_summary WHERE id = 1');
    this._updateSummaryStmt = this.db.prepare(`
      UPDATE trade_summary SET
        totalTrades = @totalTrades,
        wins = @wins,
        losses = @losses,
        totalPnL = @totalPnL,
        winRate = @winRate,
        updatedAt = datetime('now')
      WHERE id = 1
    `);

    this._getMetaStmt = this.db.prepare('SELECT * FROM trade_meta WHERE id = 1');
    this._updateMetaStmt = this.db.prepare(`
      UPDATE trade_meta SET
        realizedOffset = @realizedOffset,
        updatedAt = datetime('now')
      WHERE id = 1
    `);

    this._countStmt = this.db.prepare('SELECT COUNT(*) as count FROM trades');
    this._countClosedStmt = this.db.prepare("SELECT COUNT(*) as count FROM trades WHERE status = 'CLOSED'");
  }

  // ─── Trade CRUD ────────────────────────────────────────────────

  /**
   * Insert or replace a trade record.
   * Accepts a trade object from the JSON ledger format and normalizes it.
   * @param {Object} trade
   * @param {string} [mode='paper']
   */
  insertTrade(trade, mode = 'paper') {
    if (!trade) return;

    const params = this._normalizeTradeToParams(trade, mode);
    this._insertStmt.run(params);
  }

  /**
   * Insert multiple trades in a transaction (for migration).
   * @param {Object[]} trades
   * @param {string} [mode='paper']
   */
  insertMany(trades, mode = 'paper') {
    if (!Array.isArray(trades) || trades.length === 0) return;

    const insertMany = this.db.transaction((tradeList) => {
      for (const trade of tradeList) {
        const params = this._normalizeTradeToParams(trade, mode);
        this._insertStmt.run(params);
      }
    });

    insertMany(trades);
  }

  /**
   * Update an existing trade (e.g., close it).
   * @param {string} tradeId
   * @param {Object} updateData
   */
  updateTrade(tradeId, updateData) {
    if (!tradeId || !updateData) return;

    const params = {
      id: tradeId,
      status: updateData.status ?? null,
      exitPrice: this._toNum(updateData.exitPrice),
      pnl: this._toNum(updateData.pnl),
      exitTime: updateData.exitTime ?? null,
      exitReason: updateData.exitReason ?? null,
      maxUnrealizedPnl: this._toNum(updateData.maxUnrealizedPnl),
      minUnrealizedPnl: this._toNum(updateData.minUnrealizedPnl),
      btcSpotAtExit: this._toNum(updateData.btcSpotAtExit),
      rsiAtExit: this._toNum(updateData.rsiAtExit),
      macdHistAtExit: this._toNum(updateData.macdHistAtExit),
      vwapSlopeAtExit: this._toNum(updateData.vwapSlopeAtExit),
      modelUpAtExit: this._toNum(updateData.modelUpAtExit),
      modelDownAtExit: this._toNum(updateData.modelDownAtExit),
    };

    this._updateStmt.run(params);
  }

  /**
   * Get a trade by ID.
   * @param {string} tradeId
   * @returns {Object|null}
   */
  getTradeById(tradeId) {
    const row = this._getByIdStmt.get(tradeId);
    return row ? this._rowToTrade(row) : null;
  }

  /**
   * Get all trades (replaces loadLedger().trades).
   * @returns {Object[]}
   */
  getAllTrades() {
    return this._getAllStmt.all().map(r => this._rowToTrade(r));
  }

  /**
   * Get closed trades only.
   * @returns {Object[]}
   */
  getClosedTrades() {
    return this._getClosedStmt.all().map(r => this._rowToTrade(r));
  }

  /**
   * Get open trades.
   * @returns {Object[]}
   */
  getOpenTrades() {
    return this._getOpenStmt.all().map(r => this._rowToTrade(r));
  }

  /**
   * Get first open trade (replaces getOpenTrade()).
   * @returns {Object|null}
   */
  getFirstOpenTrade() {
    const trades = this.getOpenTrades();
    return trades.length > 0 ? trades[0] : null;
  }

  /**
   * Get trades by date range.
   * @param {string} from - ISO date string
   * @param {string} to - ISO date string
   * @returns {Object[]}
   */
  getTradesByDateRange(from, to) {
    const stmt = this.db.prepare(
      'SELECT * FROM trades WHERE timestamp >= ? AND timestamp <= ? ORDER BY timestamp ASC'
    );
    return stmt.all(from, to).map(r => this._rowToTrade(r));
  }

  /**
   * Get trades by outcome (win/loss).
   * @param {'win'|'loss'} outcome
   * @returns {Object[]}
   */
  getTradesByOutcome(outcome) {
    if (outcome === 'win') {
      const stmt = this.db.prepare("SELECT * FROM trades WHERE status = 'CLOSED' AND pnl > 0 ORDER BY timestamp ASC");
      return stmt.all().map(r => this._rowToTrade(r));
    } else if (outcome === 'loss') {
      const stmt = this.db.prepare("SELECT * FROM trades WHERE status = 'CLOSED' AND pnl < 0 ORDER BY timestamp ASC");
      return stmt.all().map(r => this._rowToTrade(r));
    }
    return [];
  }

  /**
   * Get trades by mode (paper/live).
   * @param {'paper'|'live'} mode
   * @returns {Object[]}
   */
  getTradesByMode(mode) {
    const stmt = this.db.prepare('SELECT * FROM trades WHERE mode = ? ORDER BY timestamp ASC');
    return stmt.all(mode).map(r => this._rowToTrade(r));
  }

  /**
   * Get trade count.
   * @returns {number}
   */
  getTradeCount() {
    return this._countStmt.get().count;
  }

  /**
   * Get closed trade count.
   * @returns {number}
   */
  getClosedTradeCount() {
    return this._countClosedStmt.get().count;
  }

  /**
   * Delete all trades (for testing).
   */
  deleteAll() {
    this._deleteAllStmt.run();
  }

  // ─── Summary / Meta ────────────────────────────────────────────

  /**
   * Get the trade summary (mirrors JSON ledger summary).
   * @returns {Object}
   */
  getSummary() {
    const row = this._getSummaryStmt.get();
    return {
      totalTrades: row.totalTrades,
      wins: row.wins,
      losses: row.losses,
      totalPnL: row.totalPnL,
      winRate: row.winRate,
    };
  }

  /**
   * Recalculate and update summary from all trades.
   */
  recalculateSummary() {
    const closed = this.getClosedTrades();
    let wins = 0, losses = 0, totalPnL = 0;

    for (const t of closed) {
      const pnl = typeof t.pnl === 'number' ? t.pnl : 0;
      totalPnL += pnl;
      if (pnl > 0) wins++;
      else if (pnl < 0) losses++;
    }

    const total = this.getTradeCount();
    const closedCount = wins + losses;
    const winRate = closedCount > 0 ? Number(((wins / closedCount) * 100).toFixed(2)) : 0;

    this._updateSummaryStmt.run({
      totalTrades: total,
      wins,
      losses,
      totalPnL,
      winRate,
    });
  }

  /**
   * Get the meta data (mirrors JSON ledger meta).
   * @returns {{ realizedOffset: number }}
   */
  getMeta() {
    const row = this._getMetaStmt.get();
    return { realizedOffset: row.realizedOffset };
  }

  /**
   * Update meta data.
   * @param {{ realizedOffset: number }} meta
   */
  updateMeta(meta) {
    this._updateMetaStmt.run({ realizedOffset: meta.realizedOffset ?? 0 });
  }

  // ─── Ledger-compatible interface ───────────────────────────────

  /**
   * Get data in the same shape as loadLedger() for backward compatibility.
   * @returns {{ trades: Object[], summary: Object, meta: Object }}
   */
  getLedgerData() {
    return {
      trades: this.getAllTrades(),
      summary: this.getSummary(),
      meta: this.getMeta(),
    };
  }

  // ─── Migration ─────────────────────────────────────────────────

  /**
   * Migrate trades from JSON ledger to SQLite.
   * @param {{ trades: Object[], summary: Object, meta: Object }} ledgerData
   * @param {string} [mode='paper']
   * @returns {{ migrated: number, skipped: number }}
   */
  migrateFromLedger(ledgerData, mode = 'paper') {
    if (!ledgerData || !Array.isArray(ledgerData.trades)) {
      return { migrated: 0, skipped: 0 };
    }

    const existingCount = this.getTradeCount();
    let migrated = 0;
    let skipped = 0;

    // Use transaction for atomic migration
    const migrate = this.db.transaction((trades) => {
      for (const trade of trades) {
        if (!trade || !trade.id) {
          skipped++;
          continue;
        }

        // Check if already exists
        const existing = this._getByIdStmt.get(trade.id);
        if (existing) {
          skipped++;
          continue;
        }

        this.insertTrade(trade, mode);
        migrated++;
      }
    });

    migrate(ledgerData.trades);

    // Migrate meta
    if (ledgerData.meta) {
      this.updateMeta(ledgerData.meta);
    }

    // Recalculate summary
    this.recalculateSummary();

    console.log(
      `[TradeStore] Migration complete: ${migrated} trades migrated, ${skipped} skipped ` +
      `(${existingCount} existed before migration)`
    );

    return { migrated, skipped };
  }

  // ─── Lifecycle ─────────────────────────────────────────────────

  /**
   * Close the database connection.
   */
  close() {
    if (this.db && this.db.open) {
      this.db.close();
    }
  }

  // ─── Internal helpers ──────────────────────────────────────────

  /**
   * Convert a trade object (from JSON ledger) to prepared statement params.
   */
  _normalizeTradeToParams(trade, mode) {
    // Collect known fields
    const knownFields = new Set([
      'id', 'timestamp', 'status', 'side', 'entryPrice', 'exitPrice', 'shares',
      'contractSize', 'pnl', 'entryTime', 'exitTime', 'exitReason', 'entryPhase',
      'marketSlug', 'sideInferred', 'modelProbAtEntry', 'edgeAtEntry', 'rsiAtEntry',
      'vwapDistAtEntry', 'spreadAtEntry', 'liquidityAtEntry', 'volumeNumAtEntry',
      'btcSpotAtEntry', 'spotImpulsePctAtEntry', 'heikenColorAtEntry',
      'heikenCountAtEntry', 'rangePct20AtEntry', 'recActionAtEntry',
      'macdValueAtEntry', 'macdHistAtEntry', 'macdSignalAtEntry', 'vwapSlopeAtEntry',
      'modelUpAtEntry', 'modelDownAtEntry', 'timeLeftMinAtEntry',
      'maxEntryPolyPriceAtEntry', 'btcSpotAtExit', 'rsiAtExit', 'macdHistAtExit',
      'vwapSlopeAtExit', 'modelUpAtExit', 'modelDownAtExit',
      'maxUnrealizedPnl', 'minUnrealizedPnl', 'entryGateSnapshot', 'mode',
    ]);

    // Collect extra fields into JSON blob
    const extra = {};
    for (const [k, v] of Object.entries(trade)) {
      if (!knownFields.has(k) && v !== undefined && v !== null) {
        extra[k] = v;
      }
    }

    const gateSnapshot = trade.entryGateSnapshot
      ? (typeof trade.entryGateSnapshot === 'string' ? trade.entryGateSnapshot : JSON.stringify(trade.entryGateSnapshot))
      : null;

    return {
      id: trade.id || `${Date.now()}-${Math.random().toString(36).substring(2, 7)}`,
      timestamp: trade.timestamp || new Date().toISOString(),
      status: trade.status || 'OPEN',
      side: trade.side || null,
      entryPrice: this._toNum(trade.entryPrice),
      exitPrice: this._toNum(trade.exitPrice),
      shares: this._toNum(trade.shares),
      contractSize: this._toNum(trade.contractSize),
      pnl: this._toNum(trade.pnl) ?? 0,
      entryTime: trade.entryTime || trade.timestamp || null,
      exitTime: trade.exitTime || null,
      exitReason: trade.exitReason || null,
      entryPhase: trade.entryPhase || null,
      marketSlug: trade.marketSlug || null,
      sideInferred: trade.sideInferred === true ? 1 : (trade.sideInferred === false ? 0 : null),
      modelProbAtEntry: this._toNum(trade.modelProbAtEntry),
      edgeAtEntry: this._toNum(trade.edgeAtEntry),
      rsiAtEntry: this._toNum(trade.rsiAtEntry),
      vwapDistAtEntry: this._toNum(trade.vwapDistAtEntry),
      spreadAtEntry: this._toNum(trade.spreadAtEntry),
      liquidityAtEntry: this._toNum(trade.liquidityAtEntry),
      volumeNumAtEntry: this._toNum(trade.volumeNumAtEntry),
      btcSpotAtEntry: this._toNum(trade.btcSpotAtEntry),
      spotImpulsePctAtEntry: this._toNum(trade.spotImpulsePctAtEntry),
      heikenColorAtEntry: trade.heikenColorAtEntry || null,
      heikenCountAtEntry: typeof trade.heikenCountAtEntry === 'number' ? trade.heikenCountAtEntry : null,
      rangePct20AtEntry: this._toNum(trade.rangePct20AtEntry),
      recActionAtEntry: trade.recActionAtEntry || null,
      macdValueAtEntry: this._toNum(trade.macdValueAtEntry),
      macdHistAtEntry: this._toNum(trade.macdHistAtEntry),
      macdSignalAtEntry: this._toNum(trade.macdSignalAtEntry),
      vwapSlopeAtEntry: this._toNum(trade.vwapSlopeAtEntry) ?? this._toNum(trade.vwapSlope),
      modelUpAtEntry: this._toNum(trade.modelUpAtEntry) ?? this._toNum(trade.modelUp),
      modelDownAtEntry: this._toNum(trade.modelDownAtEntry) ?? this._toNum(trade.modelDown),
      timeLeftMinAtEntry: this._toNum(trade.timeLeftMinAtEntry),
      maxEntryPolyPriceAtEntry: this._toNum(trade.maxEntryPolyPriceAtEntry),
      btcSpotAtExit: this._toNum(trade.btcSpotAtExit),
      rsiAtExit: this._toNum(trade.rsiAtExit),
      macdHistAtExit: this._toNum(trade.macdHistAtExit),
      vwapSlopeAtExit: this._toNum(trade.vwapSlopeAtExit),
      modelUpAtExit: this._toNum(trade.modelUpAtExit),
      modelDownAtExit: this._toNum(trade.modelDownAtExit),
      maxUnrealizedPnl: this._toNum(trade.maxUnrealizedPnl),
      minUnrealizedPnl: this._toNum(trade.minUnrealizedPnl),
      entryGateSnapshot: gateSnapshot,
      mode: mode || 'paper',
      extraJson: Object.keys(extra).length > 0 ? JSON.stringify(extra) : null,
    };
  }

  /**
   * Convert a DB row back to a trade object (matching JSON ledger shape).
   */
  _rowToTrade(row) {
    if (!row) return null;

    const trade = {
      id: row.id,
      timestamp: row.timestamp,
      status: row.status,
      side: row.side,
      entryPrice: row.entryPrice,
      exitPrice: row.exitPrice,
      shares: row.shares,
      contractSize: row.contractSize,
      pnl: row.pnl,
      entryTime: row.entryTime,
      exitTime: row.exitTime,
      exitReason: row.exitReason,
      entryPhase: row.entryPhase,
      marketSlug: row.marketSlug,
      sideInferred: row.sideInferred === 1 ? true : (row.sideInferred === 0 ? false : undefined),
    };

    // Add enrichment fields if present
    const enrichments = [
      'modelProbAtEntry', 'edgeAtEntry', 'rsiAtEntry', 'vwapDistAtEntry',
      'spreadAtEntry', 'liquidityAtEntry', 'volumeNumAtEntry', 'btcSpotAtEntry',
      'spotImpulsePctAtEntry', 'heikenColorAtEntry', 'heikenCountAtEntry',
      'rangePct20AtEntry', 'recActionAtEntry', 'macdValueAtEntry',
      'macdHistAtEntry', 'macdSignalAtEntry', 'vwapSlopeAtEntry',
      'modelUpAtEntry', 'modelDownAtEntry', 'timeLeftMinAtEntry',
      'maxEntryPolyPriceAtEntry', 'btcSpotAtExit', 'rsiAtExit',
      'macdHistAtExit', 'vwapSlopeAtExit', 'modelUpAtExit', 'modelDownAtExit',
      'maxUnrealizedPnl', 'minUnrealizedPnl',
    ];

    for (const field of enrichments) {
      if (row[field] !== null && row[field] !== undefined) {
        trade[field] = row[field];
      }
    }

    // Parse entry gate snapshot
    if (row.entryGateSnapshot) {
      try {
        trade.entryGateSnapshot = JSON.parse(row.entryGateSnapshot);
      } catch {
        trade.entryGateSnapshot = row.entryGateSnapshot;
      }
    }

    // Merge extra JSON fields
    if (row.extraJson) {
      try {
        const extra = JSON.parse(row.extraJson);
        Object.assign(trade, extra);
      } catch {
        // ignore malformed extra
      }
    }

    return trade;
  }

  /**
   * Safely convert to number or null.
   */
  _toNum(val) {
    if (val === null || val === undefined) return null;
    const n = Number(val);
    return Number.isFinite(n) ? n : null;
  }
}

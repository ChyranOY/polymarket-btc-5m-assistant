/**
 * @file Crash recovery state manager — PID lock + critical state persistence.
 *
 * Manages:
 *   1. PID lock file — detect crashes (stale PID = previous crash)
 *   2. Critical state file — persist kill-switch, daily PnL, circuit breaker
 *   3. State restoration on startup
 *
 * Architecture: Infrastructure layer (file I/O).
 *
 * State persisted (critical only):
 *   - kill-switch state (active, overrideActive, overrideCount, lastResetDate)
 *   - daily PnL (todayRealizedPnl)
 *   - circuit breaker (consecutiveLosses, circuitBreakerTrippedAtMs)
 *   - open position tracking (hasOpenPosition)
 *
 * NOT persisted (short-lived, safe to reset):
 *   - cooldowns (lastLossAtMs, lastWinAtMs, lastFlipAtMs)
 *   - MFE/MAE tracking (per-position, reset on close)
 *   - grace window state
 *   - blocker frequency counts
 */

import fs from 'node:fs';
import path from 'node:path';

// ── Default paths ──────────────────────────────────────────────────
const DEFAULT_DATA_DIR = process.env.DATA_DIR || './data';
const DEFAULT_PID_PATH = path.join(DEFAULT_DATA_DIR, '.pid');
const DEFAULT_STATE_PATH = path.join(DEFAULT_DATA_DIR, 'state.json');

// ── Singleton ──────────────────────────────────────────────────────
let _instance = null;

/**
 * Get the singleton StateManager instance.
 * @param {Object} [opts]
 * @returns {StateManager}
 */
export function getStateManager(opts = {}) {
  if (_instance && !opts._forceNew) return _instance;
  const mgr = new StateManager(opts);
  if (!opts._forceNew) _instance = mgr;
  return mgr;
}

/**
 * Reset singleton (for testing).
 */
export function resetStateManager() {
  _instance = null;
}

// ── StateManager class ─────────────────────────────────────────────

export class StateManager {
  /**
   * @param {Object} [opts]
   * @param {string} [opts.dataDir] - Override data directory
   * @param {string} [opts.pidPath] - Override PID file path
   * @param {string} [opts.statePath] - Override state file path
   */
  constructor(opts = {}) {
    this.dataDir = opts.dataDir || DEFAULT_DATA_DIR;
    this.pidPath = opts.pidPath || DEFAULT_PID_PATH;
    this.statePath = opts.statePath || DEFAULT_STATE_PATH;

    // Ensure data directory exists
    if (!fs.existsSync(this.dataDir)) {
      fs.mkdirSync(this.dataDir, { recursive: true });
    }

    // Debounce state writes (avoid excessive disk I/O)
    this._lastWriteMs = 0;
    this._writeDebounceMs = opts.writeDebounceMs || 5000; // 5s default
    this._pendingState = null;
    this._writeTimer = null;
  }

  // ─── PID Lock ────────────────────────────────────────────────

  /**
   * Write the current PID to the lock file.
   * @returns {void}
   */
  writePidLock() {
    try {
      fs.writeFileSync(this.pidPath, String(process.pid), 'utf8');
    } catch (err) {
      console.error('[StateManager] Failed to write PID lock:', err.message);
    }
  }

  /**
   * Remove the PID lock file (clean shutdown).
   * @returns {void}
   */
  removePidLock() {
    try {
      if (fs.existsSync(this.pidPath)) {
        fs.unlinkSync(this.pidPath);
      }
    } catch (err) {
      console.error('[StateManager] Failed to remove PID lock:', err.message);
    }
  }

  /**
   * Check if a previous PID lock exists (indicates crash).
   * If the PID file exists and the process is dead, assume crash.
   *
   * @returns {{ crashed: boolean, previousPid: number|null }}
   */
  checkForCrash() {
    try {
      if (!fs.existsSync(this.pidPath)) {
        return { crashed: false, previousPid: null };
      }

      const pidStr = fs.readFileSync(this.pidPath, 'utf8').trim();
      const previousPid = parseInt(pidStr, 10);

      if (!Number.isFinite(previousPid) || previousPid <= 0) {
        // Invalid PID file — treat as crash
        return { crashed: true, previousPid: null };
      }

      // Check if the process is still running
      try {
        process.kill(previousPid, 0); // Signal 0 = check existence only
        // Process is still alive — NOT a crash, another instance is running
        return { crashed: false, previousPid };
      } catch {
        // Process doesn't exist — previous instance crashed
        return { crashed: true, previousPid };
      }
    } catch (err) {
      console.error('[StateManager] Error checking for crash:', err.message);
      return { crashed: false, previousPid: null };
    }
  }

  // ─── State Persistence ───────────────────────────────────────

  /**
   * Persist critical trading state to JSON file.
   * Debounced to avoid excessive writes.
   *
   * @param {Object} state - TradingState instance or state snapshot
   * @param {Object} [opts]
   * @param {boolean} [opts.immediate] - Skip debounce (for shutdown)
   */
  persistState(state, opts = {}) {
    const snapshot = this._extractCriticalState(state);

    if (opts.immediate) {
      this._writeState(snapshot);
      return;
    }

    this._pendingState = snapshot;
    const now = Date.now();

    if (now - this._lastWriteMs >= this._writeDebounceMs) {
      this._writeState(snapshot);
      this._pendingState = null;
    } else if (!this._writeTimer) {
      this._writeTimer = setTimeout(() => {
        if (this._pendingState) {
          this._writeState(this._pendingState);
          this._pendingState = null;
        }
        this._writeTimer = null;
      }, this._writeDebounceMs);
    }
  }

  /**
   * Load persisted state from JSON file.
   * @returns {Object|null} State snapshot or null if not found
   */
  loadState() {
    try {
      if (!fs.existsSync(this.statePath)) {
        return null;
      }

      const data = fs.readFileSync(this.statePath, 'utf8');
      const parsed = JSON.parse(data);

      // Validate basic structure
      if (!parsed || typeof parsed !== 'object') {
        console.warn('[StateManager] Invalid state file format');
        return null;
      }

      console.log('[StateManager] Loaded persisted state:', {
        killSwitchActive: parsed.killSwitch?.active ?? false,
        todayPnl: parsed.todayRealizedPnl ?? 0,
        consecutiveLosses: parsed.consecutiveLosses ?? 0,
        savedAt: parsed._savedAt ?? 'unknown',
      });

      return parsed;
    } catch (err) {
      console.error('[StateManager] Failed to load state:', err.message);
      return null;
    }
  }

  /**
   * Restore persisted state into a TradingState instance.
   *
   * @param {import('../../application/TradingState.js').TradingState} tradingState
   * @param {Object} [persistedState] - Pre-loaded state (or null to load from file)
   * @returns {boolean} True if state was restored
   */
  restoreState(tradingState, persistedState = null) {
    const state = persistedState || this.loadState();
    if (!state) return false;

    try {
      // Restore kill-switch state
      if (state.killSwitch && typeof state.killSwitch === 'object') {
        tradingState.killSwitchState = {
          active: Boolean(state.killSwitch.active),
          overrideActive: Boolean(state.killSwitch.overrideActive),
          overrideCount: Number(state.killSwitch.overrideCount) || 0,
          overrideLog: Array.isArray(state.killSwitch.overrideLog) ? state.killSwitch.overrideLog : [],
          lastResetDate: state.killSwitch.lastResetDate ?? null,
        };
      }

      // Restore daily PnL
      if (typeof state.todayRealizedPnl === 'number' && Number.isFinite(state.todayRealizedPnl)) {
        tradingState.todayRealizedPnl = state.todayRealizedPnl;
      }

      // Restore circuit breaker
      if (typeof state.consecutiveLosses === 'number') {
        tradingState.consecutiveLosses = state.consecutiveLosses;
      }
      if (typeof state.circuitBreakerTrippedAtMs === 'number') {
        tradingState.circuitBreakerTrippedAtMs = state.circuitBreakerTrippedAtMs;
      }

      // Restore open position flag
      if (typeof state.hasOpenPosition === 'boolean') {
        tradingState.hasOpenPosition = state.hasOpenPosition;
      }

      // Restore today key
      if (state._todayKey) {
        tradingState._todayKey = state._todayKey;
      }

      console.log('[StateManager] State restored successfully');
      return true;
    } catch (err) {
      console.error('[StateManager] Failed to restore state:', err.message);
      return false;
    }
  }

  /**
   * Clear the persisted state file.
   */
  clearState() {
    try {
      if (fs.existsSync(this.statePath)) {
        fs.unlinkSync(this.statePath);
      }
    } catch (err) {
      console.error('[StateManager] Failed to clear state:', err.message);
    }
  }

  // ─── Lifecycle ───────────────────────────────────────────────

  /**
   * Full startup sequence:
   *   1. Check for crash
   *   2. Load persisted state
   *   3. Write new PID lock
   *
   * @returns {{ crashed: boolean, previousPid: number|null, restoredState: Object|null }}
   */
  startup() {
    const crashCheck = this.checkForCrash();
    let restoredState = null;

    if (crashCheck.crashed) {
      console.warn(
        `[StateManager] CRASH DETECTED — previous PID: ${crashCheck.previousPid ?? 'unknown'}. ` +
        'Attempting state recovery...'
      );
      restoredState = this.loadState();
    }

    // Write current PID
    this.writePidLock();

    return {
      crashed: crashCheck.crashed,
      previousPid: crashCheck.previousPid,
      restoredState,
    };
  }

  /**
   * Clean shutdown sequence:
   *   1. Persist final state
   *   2. Remove PID lock
   *   3. Clear timers
   */
  shutdown(tradingState) {
    // Cancel pending debounced write
    if (this._writeTimer) {
      clearTimeout(this._writeTimer);
      this._writeTimer = null;
    }

    // Persist final state immediately
    if (tradingState) {
      this.persistState(tradingState, { immediate: true });
    }

    // Remove PID lock (clean shutdown)
    this.removePidLock();
  }

  // ─── Internal ────────────────────────────────────────────────

  /**
   * Extract critical state fields for persistence.
   */
  _extractCriticalState(state) {
    if (!state) return {};

    return {
      killSwitch: {
        active: Boolean(state.killSwitchState?.active),
        overrideActive: Boolean(state.killSwitchState?.overrideActive),
        overrideCount: state.killSwitchState?.overrideCount ?? 0,
        overrideLog: state.killSwitchState?.overrideLog ?? [],
        lastResetDate: state.killSwitchState?.lastResetDate ?? null,
      },
      todayRealizedPnl: state.todayRealizedPnl ?? 0,
      consecutiveLosses: state.consecutiveLosses ?? 0,
      circuitBreakerTrippedAtMs: state.circuitBreakerTrippedAtMs ?? null,
      hasOpenPosition: state.hasOpenPosition ?? false,
      _todayKey: state._todayKey ?? null,
      _savedAt: new Date().toISOString(),
      _pid: process.pid,
    };
  }

  /**
   * Write state to file synchronously.
   */
  _writeState(snapshot) {
    try {
      const json = JSON.stringify(snapshot, null, 2);
      fs.writeFileSync(this.statePath, json, 'utf8');
      this._lastWriteMs = Date.now();
    } catch (err) {
      console.error('[StateManager] Failed to write state:', err.message);
    }
  }
}

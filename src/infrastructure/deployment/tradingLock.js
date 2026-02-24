/**
 * @file Trading lock — file-based instance coordination for zero-downtime deployment.
 *
 * Only the lock holder can execute trades. New instances wait for lock release
 * or stale heartbeat (>30s). Heartbeat is updated periodically by the holder.
 *
 * Architecture: Infrastructure layer (file I/O).
 *
 * Lock file format (trading.lock):
 *   {
 *     "instanceId": "<hex>",
 *     "pid": <number>,
 *     "heartbeat": <timestamp_ms>,
 *     "acquiredAt": "<ISO string>"
 *   }
 *
 * Behavior:
 *   - acquireLock() — attempt to acquire. Succeeds if no lock or stale heartbeat.
 *   - updateHeartbeat() — refresh heartbeat timestamp (call every 10s).
 *   - releaseLock() — remove lock file (clean shutdown).
 *   - isLockHolder() — check if this instance holds the lock.
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';

// ── Defaults ───────────────────────────────────────────────────────
const DEFAULT_DATA_DIR = process.env.DATA_DIR || './data';
const DEFAULT_LOCK_PATH = path.join(DEFAULT_DATA_DIR, 'trading.lock');
const DEFAULT_STALE_THRESHOLD_MS = 30_000; // 30s
const DEFAULT_HEARTBEAT_INTERVAL_MS = 10_000; // 10s

// ── Singleton ──────────────────────────────────────────────────────
let _instance = null;

/**
 * Get the singleton TradingLock instance.
 * @param {Object} [opts]
 * @returns {TradingLock}
 */
export function getTradingLock(opts = {}) {
  if (_instance && !opts._forceNew) return _instance;
  const lock = new TradingLock(opts);
  if (!opts._forceNew) _instance = lock;
  return lock;
}

/**
 * Reset singleton (for testing).
 */
export function resetTradingLock() {
  if (_instance) {
    _instance.stopHeartbeat();
  }
  _instance = null;
}

// ── TradingLock class ──────────────────────────────────────────────

export class TradingLock {
  /**
   * @param {Object} [opts]
   * @param {string} [opts.lockPath] - Path to lock file
   * @param {number} [opts.staleThresholdMs] - When heartbeat is considered stale
   * @param {number} [opts.heartbeatIntervalMs] - Heartbeat update frequency
   * @param {string} [opts.instanceId] - Override instance ID (for testing)
   */
  constructor(opts = {}) {
    this.lockPath = opts.lockPath || DEFAULT_LOCK_PATH;
    this.staleThresholdMs = opts.staleThresholdMs || DEFAULT_STALE_THRESHOLD_MS;
    this.heartbeatIntervalMs = opts.heartbeatIntervalMs || DEFAULT_HEARTBEAT_INTERVAL_MS;
    this.instanceId = opts.instanceId || crypto.randomBytes(4).toString('hex');

    this._heartbeatTimer = null;
    this._isHolder = false;

    // Ensure data directory exists
    const dir = path.dirname(this.lockPath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }
  }

  // ─── Lock operations ─────────────────────────────────────────

  /**
   * Attempt to acquire the trading lock.
   *
   * Succeeds if:
   *   - No lock file exists
   *   - Lock file exists but heartbeat is stale (>30s)
   *   - Lock file exists and belongs to this instance
   *
   * @returns {{ acquired: boolean, reason: string, holderId?: string }}
   */
  acquireLock() {
    try {
      const existing = this._readLock();

      if (existing) {
        // Already ours
        if (existing.instanceId === this.instanceId) {
          this._isHolder = true;
          this._startHeartbeat();
          return { acquired: true, reason: 'already_held' };
        }

        // Check if stale
        const elapsed = Date.now() - (existing.heartbeat || 0);
        if (elapsed < this.staleThresholdMs) {
          return {
            acquired: false,
            reason: 'held_by_other',
            holderId: existing.instanceId,
          };
        }

        // Stale — we can take over
        console.warn(
          `[TradingLock] Taking over stale lock from ${existing.instanceId} ` +
          `(last heartbeat ${(elapsed / 1000).toFixed(1)}s ago)`
        );
      }

      // Acquire
      this._writeLock();
      this._isHolder = true;
      this._startHeartbeat();
      return { acquired: true, reason: existing ? 'takeover_stale' : 'new_lock' };
    } catch (err) {
      console.error('[TradingLock] Failed to acquire lock:', err.message);
      return { acquired: false, reason: 'error' };
    }
  }

  /**
   * Release the trading lock (clean shutdown).
   * @returns {boolean} True if released
   */
  releaseLock() {
    this.stopHeartbeat();
    this._isHolder = false;

    try {
      if (fs.existsSync(this.lockPath)) {
        // Only delete if we hold it
        const existing = this._readLock();
        if (existing && existing.instanceId === this.instanceId) {
          fs.unlinkSync(this.lockPath);
          console.log('[TradingLock] Lock released');
          return true;
        }
      }
      return false;
    } catch (err) {
      console.error('[TradingLock] Failed to release lock:', err.message);
      return false;
    }
  }

  /**
   * Check if this instance currently holds the lock.
   * @returns {boolean}
   */
  isLockHolder() {
    if (!this._isHolder) return false;

    // Verify from file (in case another process took over)
    try {
      const existing = this._readLock();
      if (!existing || existing.instanceId !== this.instanceId) {
        this._isHolder = false;
        this.stopHeartbeat();
        return false;
      }
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Wait for the lock to become available (with timeout).
   *
   * @param {number} [timeoutMs=35000] - Max wait time
   * @param {number} [pollIntervalMs=2000] - Check frequency
   * @returns {Promise<{ acquired: boolean, reason: string }>}
   */
  async waitForLock(timeoutMs = 35_000, pollIntervalMs = 2000) {
    const deadline = Date.now() + timeoutMs;

    while (Date.now() < deadline) {
      const result = this.acquireLock();
      if (result.acquired) return result;

      await new Promise(resolve => setTimeout(resolve, pollIntervalMs));
    }

    // Try one last time
    return this.acquireLock();
  }

  // ─── Heartbeat ────────────────────────────────────────────────

  /**
   * Update the heartbeat timestamp.
   */
  updateHeartbeat() {
    if (!this._isHolder) return;

    try {
      const existing = this._readLock();
      if (existing && existing.instanceId === this.instanceId) {
        existing.heartbeat = Date.now();
        fs.writeFileSync(this.lockPath, JSON.stringify(existing, null, 2), 'utf8');
      }
    } catch (err) {
      console.warn('[TradingLock] Heartbeat update failed:', err.message);
    }
  }

  /**
   * Start the heartbeat interval.
   */
  _startHeartbeat() {
    this.stopHeartbeat();
    this._heartbeatTimer = setInterval(() => {
      this.updateHeartbeat();
    }, this.heartbeatIntervalMs);

    // Unref so it doesn't prevent process exit
    if (this._heartbeatTimer?.unref) {
      this._heartbeatTimer.unref();
    }
  }

  /**
   * Stop the heartbeat interval.
   */
  stopHeartbeat() {
    if (this._heartbeatTimer) {
      clearInterval(this._heartbeatTimer);
      this._heartbeatTimer = null;
    }
  }

  // ─── Status ───────────────────────────────────────────────────

  /**
   * Get lock status (for health endpoint / diagnostics).
   * @returns {Object}
   */
  getStatus() {
    const existing = this._readLock();

    return {
      isHolder: this._isHolder,
      instanceId: this.instanceId,
      lockExists: existing !== null,
      lockHolder: existing?.instanceId ?? null,
      heartbeatAge: existing
        ? Date.now() - (existing.heartbeat || 0)
        : null,
      staleThresholdMs: this.staleThresholdMs,
      isStale: existing
        ? (Date.now() - (existing.heartbeat || 0) > this.staleThresholdMs)
        : false,
    };
  }

  // ─── Internal ─────────────────────────────────────────────────

  /**
   * Read the lock file.
   * @returns {Object|null}
   */
  _readLock() {
    try {
      if (!fs.existsSync(this.lockPath)) return null;
      const data = fs.readFileSync(this.lockPath, 'utf8');
      return JSON.parse(data);
    } catch {
      return null;
    }
  }

  /**
   * Write the lock file with this instance's data.
   */
  _writeLock() {
    const lockData = {
      instanceId: this.instanceId,
      pid: process.pid,
      heartbeat: Date.now(),
      acquiredAt: new Date().toISOString(),
    };

    fs.writeFileSync(this.lockPath, JSON.stringify(lockData, null, 2), 'utf8');
  }
}

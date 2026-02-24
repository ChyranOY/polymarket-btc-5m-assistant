/**
 * @file Kill-switch domain logic — pure functions.
 *
 * Manages the daily PnL kill-switch: activation check, override, midnight reset.
 * All functions are pure — they take state and return new state or check results.
 * No I/O, no side effects.
 *
 * Applies to BOTH paper and live modes (per user decision).
 */

/**
 * Create initial kill-switch state.
 * @returns {Object}
 */
export function createKillSwitchState() {
  return {
    active: false,
    overrideActive: false,
    overrideCount: 0,
    overrideLog: [],
    lastResetDate: null,
  };
}

/**
 * Check if the kill-switch should be triggered.
 *
 * @param {Object} state - Kill-switch state from createKillSwitchState()
 * @param {number} todayRealizedPnl - Today's cumulative realized PnL (negative = loss)
 * @param {number} maxDailyLossUsd - Configured daily loss limit (positive number, e.g., 50)
 * @param {Object} [opts]
 * @param {number} [opts.overrideBufferPct=0.10] - Additional loss buffer after override (10%)
 * @returns {{ triggered: boolean, reason?: string, overridden?: boolean }}
 */
export function checkKillSwitch(state, todayRealizedPnl, maxDailyLossUsd, opts = {}) {
  // Kill-switch disabled if limit is 0, null, or not a number
  if (!maxDailyLossUsd || typeof maxDailyLossUsd !== 'number' || maxDailyLossUsd <= 0) {
    return { triggered: false };
  }

  if (typeof todayRealizedPnl !== 'number' || !Number.isFinite(todayRealizedPnl)) {
    return { triggered: false };
  }

  const lossThreshold = -Math.abs(maxDailyLossUsd);

  // If override is active, allow additional buffer before re-triggering
  if (state.overrideActive) {
    const bufferPct = opts.overrideBufferPct ?? 0.10;
    const overrideThreshold = lossThreshold * (1 + bufferPct);

    if (todayRealizedPnl <= overrideThreshold) {
      return {
        triggered: true,
        reason: `Daily loss $${todayRealizedPnl.toFixed(2)} exceeded override threshold $${overrideThreshold.toFixed(2)}`,
      };
    }

    return { triggered: false, overridden: true };
  }

  // Standard check
  if (todayRealizedPnl <= lossThreshold) {
    return {
      triggered: true,
      reason: `Daily loss $${todayRealizedPnl.toFixed(2)} hit limit $${lossThreshold.toFixed(2)}`,
    };
  }

  return { triggered: false };
}

/**
 * Activate the kill-switch override.
 * Does NOT clear todayRealizedPnl — losses persist, just allows more trading.
 *
 * @param {Object} state - Current kill-switch state
 * @returns {Object} New state with override active
 */
export function overrideKillSwitch(state) {
  return {
    ...state,
    overrideActive: true,
    overrideCount: state.overrideCount + 1,
    overrideLog: [
      ...state.overrideLog,
      { timestamp: new Date().toISOString(), count: state.overrideCount + 1 },
    ],
  };
}

/**
 * Check if the kill-switch should reset (midnight Pacific time boundary).
 *
 * @param {Object} state - Kill-switch state
 * @param {Date} [nowDate] - Current date (defaults to now)
 * @param {string} [timezone='America/Los_Angeles'] - Timezone for reset
 * @returns {boolean} True if a new day has started since last reset
 */
export function shouldResetKillSwitch(state, nowDate, timezone = 'America/Los_Angeles') {
  const now = nowDate || new Date();

  // Get current date in Pacific time
  const todayPT = new Intl.DateTimeFormat('en-CA', {
    timeZone: timezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(now); // Returns YYYY-MM-DD in en-CA locale

  if (state.lastResetDate === null) return true;
  return state.lastResetDate !== todayPT;
}

/**
 * Reset the kill-switch for a new day.
 * Clears override state but preserves override log for audit trail.
 *
 * @param {Object} state - Current kill-switch state
 * @param {Date} [nowDate] - Current date
 * @param {string} [timezone='America/Los_Angeles']
 * @returns {Object} New reset state
 */
export function resetKillSwitch(state, nowDate, timezone = 'America/Los_Angeles') {
  const now = nowDate || new Date();
  const todayPT = new Intl.DateTimeFormat('en-CA', {
    timeZone: timezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(now);

  return {
    ...state,
    active: false,
    overrideActive: false,
    overrideCount: 0,
    lastResetDate: todayPT,
    // overrideLog preserved for audit trail
  };
}

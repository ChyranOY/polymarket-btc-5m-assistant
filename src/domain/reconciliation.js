/**
 * @file Position reconciliation — pure domain logic.
 *
 * Compares local position tracking with CLOB API state.
 * Reports discrepancies but NEVER auto-corrects.
 *
 * Pure function — no I/O, no side effects.
 */

export const SYNC_STATUS = {
  IN_SYNC: 'in_sync',
  CHECKING: 'checking',
  DISCREPANCY: 'discrepancy',
};

export const DISCREPANCY_TYPES = {
  LOCAL_ONLY: 'LOCAL_ONLY',       // Local thinks we have a position, CLOB disagrees
  CLOB_ONLY: 'CLOB_ONLY',        // CLOB has a position we don't track locally
  QTY_MISMATCH: 'QTY_MISMATCH',  // Both have the position but quantities differ
  SIDE_MISMATCH: 'SIDE_MISMATCH', // Both have the position but sides differ
};

/**
 * Reconcile local position state with CLOB API positions.
 *
 * @param {Array<{tokenID: string, qty: number, side?: string, createdAtMs?: number}>} localPositions
 *   Positions from local tracking (OrderManager / LiveExecutor).
 * @param {Array<{tokenID: string, qty: number, side?: string}>} clobPositions
 *   Positions computed from CLOB API trades.
 * @param {Object} [opts]
 * @param {number} [opts.graceWindowMs=10000] - Ignore orders placed within this window to avoid timing false positives.
 * @param {number} [opts.qtyTolerance=0] - Allow small qty differences (absolute).
 * @param {number} [opts.nowMs] - Current time in ms (for testing; defaults to Date.now()).
 * @returns {{ status: string, discrepancies: Array<{type: string, tokenID: string, local: Object|null, clob: Object|null, detail: string}> }}
 */
export function reconcilePositions(localPositions, clobPositions, opts = {}) {
  const graceWindowMs = opts.graceWindowMs ?? 10_000;
  const qtyTolerance = opts.qtyTolerance ?? 0;
  const nowMs = opts.nowMs ?? Date.now();

  const local = Array.isArray(localPositions) ? localPositions : [];
  const clob = Array.isArray(clobPositions) ? clobPositions : [];

  const discrepancies = [];

  // Build maps by tokenID
  const localMap = new Map();
  for (const pos of local) {
    if (!pos.tokenID) continue;
    localMap.set(pos.tokenID, pos);
  }

  const clobMap = new Map();
  for (const pos of clob) {
    if (!pos.tokenID) continue;
    clobMap.set(pos.tokenID, pos);
  }

  // Check local positions against CLOB
  for (const [tokenID, localPos] of localMap) {
    // Skip positions within grace window (recently created, may not appear in CLOB yet)
    if (localPos.createdAtMs && (nowMs - localPos.createdAtMs < graceWindowMs)) {
      continue;
    }

    const clobPos = clobMap.get(tokenID);

    if (!clobPos) {
      discrepancies.push({
        type: DISCREPANCY_TYPES.LOCAL_ONLY,
        tokenID,
        local: { qty: localPos.qty, side: localPos.side ?? null },
        clob: null,
        detail: `Local tracks ${localPos.qty} shares but CLOB has no position`,
      });
      continue;
    }

    // Check quantity mismatch
    const qtyDiff = Math.abs((localPos.qty || 0) - (clobPos.qty || 0));
    if (qtyDiff > qtyTolerance) {
      discrepancies.push({
        type: DISCREPANCY_TYPES.QTY_MISMATCH,
        tokenID,
        local: { qty: localPos.qty, side: localPos.side ?? null },
        clob: { qty: clobPos.qty, side: clobPos.side ?? null },
        detail: `Qty mismatch: local=${localPos.qty}, clob=${clobPos.qty} (diff=${qtyDiff})`,
      });
    }

    // Check side mismatch (if both have side info)
    if (localPos.side && clobPos.side &&
        localPos.side.toUpperCase() !== clobPos.side.toUpperCase()) {
      discrepancies.push({
        type: DISCREPANCY_TYPES.SIDE_MISMATCH,
        tokenID,
        local: { qty: localPos.qty, side: localPos.side },
        clob: { qty: clobPos.qty, side: clobPos.side },
        detail: `Side mismatch: local=${localPos.side}, clob=${clobPos.side}`,
      });
    }
  }

  // Check CLOB positions not in local
  for (const [tokenID, clobPos] of clobMap) {
    if (!localMap.has(tokenID)) {
      discrepancies.push({
        type: DISCREPANCY_TYPES.CLOB_ONLY,
        tokenID,
        local: null,
        clob: { qty: clobPos.qty, side: clobPos.side ?? null },
        detail: `CLOB has ${clobPos.qty} shares but local has no tracking`,
      });
    }
  }

  return {
    status: discrepancies.length === 0 ? SYNC_STATUS.IN_SYNC : SYNC_STATUS.DISCREPANCY,
    discrepancies,
  };
}

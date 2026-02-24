import test from 'node:test';
import assert from 'node:assert/strict';

import { reconcilePositions, SYNC_STATUS, DISCREPANCY_TYPES } from '../../src/domain/reconciliation.js';

// ── In-sync scenarios ──────────────────────────────────────────────

test('reconciliation: matching positions returns in_sync', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];
  const clob = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

test('reconciliation: empty local + empty CLOB returns in_sync', () => {
  const result = reconcilePositions([], []);
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

test('reconciliation: multiple matching positions returns in_sync', () => {
  const local = [
    { tokenID: 'tok1', qty: 100, side: 'UP' },
    { tokenID: 'tok2', qty: 50, side: 'DOWN' },
  ];
  const clob = [
    { tokenID: 'tok1', qty: 100, side: 'UP' },
    { tokenID: 'tok2', qty: 50, side: 'DOWN' },
  ];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

// ── Discrepancy scenarios ──────────────────────────────────────────

test('reconciliation: local has position CLOB does not -> LOCAL_ONLY', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];
  const clob = [];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  assert.equal(result.discrepancies.length, 1);
  assert.equal(result.discrepancies[0].type, DISCREPANCY_TYPES.LOCAL_ONLY);
  assert.equal(result.discrepancies[0].tokenID, 'tok1');
  assert.equal(result.discrepancies[0].clob, null);
  assert.equal(result.discrepancies[0].local.qty, 100);
});

test('reconciliation: CLOB has position local does not -> CLOB_ONLY', () => {
  const local = [];
  const clob = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  assert.equal(result.discrepancies.length, 1);
  assert.equal(result.discrepancies[0].type, DISCREPANCY_TYPES.CLOB_ONLY);
  assert.equal(result.discrepancies[0].tokenID, 'tok1');
  assert.equal(result.discrepancies[0].local, null);
  assert.equal(result.discrepancies[0].clob.qty, 100);
});

test('reconciliation: qty mismatch -> QTY_MISMATCH', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];
  const clob = [{ tokenID: 'tok1', qty: 80, side: 'UP' }];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  assert.equal(result.discrepancies.length, 1);
  assert.equal(result.discrepancies[0].type, DISCREPANCY_TYPES.QTY_MISMATCH);
  assert.ok(result.discrepancies[0].detail.includes('100'));
  assert.ok(result.discrepancies[0].detail.includes('80'));
});

test('reconciliation: side mismatch -> SIDE_MISMATCH', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];
  const clob = [{ tokenID: 'tok1', qty: 100, side: 'DOWN' }];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  assert.ok(result.discrepancies.some(d => d.type === DISCREPANCY_TYPES.SIDE_MISMATCH));
});

test('reconciliation: multiple discrepancies reported', () => {
  const local = [
    { tokenID: 'tok1', qty: 100, side: 'UP' },
    { tokenID: 'tok2', qty: 50, side: 'DOWN' },
  ];
  const clob = [
    { tokenID: 'tok1', qty: 80, side: 'UP' },  // qty mismatch
    { tokenID: 'tok3', qty: 30, side: 'UP' },   // CLOB-only
  ];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  // tok1: QTY_MISMATCH, tok2: LOCAL_ONLY, tok3: CLOB_ONLY
  assert.equal(result.discrepancies.length, 3);
  assert.ok(result.discrepancies.some(d => d.type === DISCREPANCY_TYPES.QTY_MISMATCH));
  assert.ok(result.discrepancies.some(d => d.type === DISCREPANCY_TYPES.LOCAL_ONLY));
  assert.ok(result.discrepancies.some(d => d.type === DISCREPANCY_TYPES.CLOB_ONLY));
});

// ── Grace window ───────────────────────────────────────────────────

test('reconciliation: grace window filters out recent local orders', () => {
  const now = Date.now();
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP', createdAtMs: now - 5000 }]; // 5s ago
  const clob = []; // CLOB hasn't caught up yet

  // Grace window is 10s -> should NOT flag as discrepancy
  const result = reconcilePositions(local, clob, { graceWindowMs: 10_000, nowMs: now });
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

test('reconciliation: grace window does NOT filter old local orders', () => {
  const now = Date.now();
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP', createdAtMs: now - 15000 }]; // 15s ago
  const clob = []; // CLOB still doesn't have it

  // Grace window is 10s -> SHOULD flag as discrepancy
  const result = reconcilePositions(local, clob, { graceWindowMs: 10_000, nowMs: now });
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  assert.equal(result.discrepancies.length, 1);
  assert.equal(result.discrepancies[0].type, DISCREPANCY_TYPES.LOCAL_ONLY);
});

// ── Qty tolerance ──────────────────────────────────────────────────

test('reconciliation: qtyTolerance allows small differences', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];
  const clob = [{ tokenID: 'tok1', qty: 99, side: 'UP' }];

  const result = reconcilePositions(local, clob, { qtyTolerance: 2 });
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

test('reconciliation: qtyTolerance exceeded triggers mismatch', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];
  const clob = [{ tokenID: 'tok1', qty: 95, side: 'UP' }];

  const result = reconcilePositions(local, clob, { qtyTolerance: 2 });
  assert.equal(result.status, SYNC_STATUS.DISCREPANCY);
  assert.equal(result.discrepancies[0].type, DISCREPANCY_TYPES.QTY_MISMATCH);
});

// ── Edge cases ─────────────────────────────────────────────────────

test('reconciliation: null/undefined inputs treated as empty arrays', () => {
  const result = reconcilePositions(null, undefined);
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

test('reconciliation: positions without tokenID are skipped', () => {
  const local = [{ qty: 100, side: 'UP' }]; // missing tokenID
  const clob = [{ qty: 100, side: 'UP' }]; // missing tokenID

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
  assert.equal(result.discrepancies.length, 0);
});

test('reconciliation: case-insensitive side comparison', () => {
  const local = [{ tokenID: 'tok1', qty: 100, side: 'up' }];
  const clob = [{ tokenID: 'tok1', qty: 100, side: 'UP' }];

  const result = reconcilePositions(local, clob);
  assert.equal(result.status, SYNC_STATUS.IN_SYNC);
});

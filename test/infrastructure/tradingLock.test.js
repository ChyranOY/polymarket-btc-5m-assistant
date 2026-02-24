import test from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { TradingLock } from '../../src/infrastructure/deployment/tradingLock.js';

// ── Test helpers ────────────────────────────────────────────────────

function makeTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'tradinglock-test-'));
}

function createLock(tmpDir, opts = {}) {
  const lock = new TradingLock({
    lockPath: path.join(tmpDir, 'trading.lock'),
    staleThresholdMs: opts.staleThresholdMs || 30_000,
    heartbeatIntervalMs: opts.heartbeatIntervalMs || 100_000, // Long interval for tests
    instanceId: opts.instanceId || undefined,
    _forceNew: true,
  });
  return lock;
}

function cleanupDir(tmpDir) {
  try { fs.rmSync(tmpDir, { recursive: true, force: true }); } catch { /* ignore */ }
}

// ── acquireLock ─────────────────────────────────────────────────────

test('TradingLock: acquireLock succeeds when no lock exists', () => {
  const tmpDir = makeTempDir();
  const lock = createLock(tmpDir);

  const result = lock.acquireLock();
  assert.strictEqual(result.acquired, true);
  assert.strictEqual(result.reason, 'new_lock');

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

test('TradingLock: acquireLock succeeds when lock is stale', () => {
  const tmpDir = makeTempDir();
  const lockPath = path.join(tmpDir, 'trading.lock');

  // Write a stale lock (heartbeat way in the past)
  fs.writeFileSync(lockPath, JSON.stringify({
    instanceId: 'old-instance',
    pid: 999999999,
    heartbeat: Date.now() - 60_000, // 60s ago
    acquiredAt: new Date().toISOString(),
  }), 'utf8');

  const lock = createLock(tmpDir, { staleThresholdMs: 5000 });
  const result = lock.acquireLock();
  assert.strictEqual(result.acquired, true);
  assert.strictEqual(result.reason, 'takeover_stale');

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

test('TradingLock: acquireLock fails when held by another active instance', () => {
  const tmpDir = makeTempDir();
  const lockPath = path.join(tmpDir, 'trading.lock');

  // Write a fresh lock from another instance
  fs.writeFileSync(lockPath, JSON.stringify({
    instanceId: 'other-instance',
    pid: process.pid, // Use our PID so it looks alive
    heartbeat: Date.now(),
    acquiredAt: new Date().toISOString(),
  }), 'utf8');

  const lock = createLock(tmpDir);
  const result = lock.acquireLock();
  assert.strictEqual(result.acquired, false);
  assert.strictEqual(result.reason, 'held_by_other');
  assert.strictEqual(result.holderId, 'other-instance');

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

test('TradingLock: acquireLock succeeds for same instance (already_held)', () => {
  const tmpDir = makeTempDir();
  const lock = createLock(tmpDir, { instanceId: 'my-instance' });

  const first = lock.acquireLock();
  assert.strictEqual(first.acquired, true);

  const second = lock.acquireLock();
  assert.strictEqual(second.acquired, true);
  assert.strictEqual(second.reason, 'already_held');

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

// ── releaseLock ─────────────────────────────────────────────────────

test('TradingLock: releaseLock removes lock file', () => {
  const tmpDir = makeTempDir();
  const lockPath = path.join(tmpDir, 'trading.lock');
  const lock = createLock(tmpDir);

  lock.acquireLock();
  assert.ok(fs.existsSync(lockPath));

  const released = lock.releaseLock();
  assert.strictEqual(released, true);
  assert.ok(!fs.existsSync(lockPath));

  cleanupDir(tmpDir);
});

test('TradingLock: releaseLock does not delete other instance lock', () => {
  const tmpDir = makeTempDir();
  const lockPath = path.join(tmpDir, 'trading.lock');

  // Write lock from another instance
  fs.writeFileSync(lockPath, JSON.stringify({
    instanceId: 'other-instance',
    pid: process.pid,
    heartbeat: Date.now(),
  }), 'utf8');

  const lock = createLock(tmpDir);
  const released = lock.releaseLock();
  assert.strictEqual(released, false);
  // Lock file should still exist
  assert.ok(fs.existsSync(lockPath));

  cleanupDir(tmpDir);
});

// ── isLockHolder ────────────────────────────────────────────────────

test('TradingLock: isLockHolder returns true when holding', () => {
  const tmpDir = makeTempDir();
  const lock = createLock(tmpDir);

  assert.strictEqual(lock.isLockHolder(), false);
  lock.acquireLock();
  assert.strictEqual(lock.isLockHolder(), true);

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

test('TradingLock: isLockHolder returns false after release', () => {
  const tmpDir = makeTempDir();
  const lock = createLock(tmpDir);

  lock.acquireLock();
  assert.strictEqual(lock.isLockHolder(), true);

  lock.releaseLock();
  assert.strictEqual(lock.isLockHolder(), false);

  cleanupDir(tmpDir);
});

// ── updateHeartbeat ─────────────────────────────────────────────────

test('TradingLock: updateHeartbeat refreshes timestamp', async () => {
  const tmpDir = makeTempDir();
  const lockPath = path.join(tmpDir, 'trading.lock');
  const lock = createLock(tmpDir);

  lock.acquireLock();
  const data1 = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
  const hb1 = data1.heartbeat;

  // Wait a bit and update heartbeat
  await new Promise(resolve => setTimeout(resolve, 50));
  lock.updateHeartbeat();

  const data2 = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
  const hb2 = data2.heartbeat;

  assert.ok(hb2 >= hb1);

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

// ── getStatus ───────────────────────────────────────────────────────

test('TradingLock: getStatus returns correct info', () => {
  const tmpDir = makeTempDir();
  const lock = createLock(tmpDir, { instanceId: 'status-test' });

  const beforeAcquire = lock.getStatus();
  assert.strictEqual(beforeAcquire.isHolder, false);
  assert.strictEqual(beforeAcquire.lockExists, false);
  assert.strictEqual(beforeAcquire.instanceId, 'status-test');

  lock.acquireLock();

  const afterAcquire = lock.getStatus();
  assert.strictEqual(afterAcquire.isHolder, true);
  assert.strictEqual(afterAcquire.lockExists, true);
  assert.strictEqual(afterAcquire.lockHolder, 'status-test');
  assert.ok(afterAcquire.heartbeatAge !== null);
  assert.strictEqual(afterAcquire.isStale, false);

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

// ── waitForLock ─────────────────────────────────────────────────────

test('TradingLock: waitForLock succeeds immediately when free', async () => {
  const tmpDir = makeTempDir();
  const lock = createLock(tmpDir);

  const result = await lock.waitForLock(5000, 100);
  assert.strictEqual(result.acquired, true);

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

test('TradingLock: waitForLock acquires after stale lock expires', async () => {
  const tmpDir = makeTempDir();
  const lockPath = path.join(tmpDir, 'trading.lock');

  // Write a lock that will become stale quickly
  fs.writeFileSync(lockPath, JSON.stringify({
    instanceId: 'stale-instance',
    pid: 999999999,
    heartbeat: Date.now() - 200, // Almost stale
    acquiredAt: new Date().toISOString(),
  }), 'utf8');

  const lock = createLock(tmpDir, { staleThresholdMs: 300 }); // 300ms stale threshold
  const result = await lock.waitForLock(2000, 100);
  assert.strictEqual(result.acquired, true);

  lock.stopHeartbeat();
  cleanupDir(tmpDir);
});

// ── Concurrent lock coordination ────────────────────────────────────

test('TradingLock: two instances coordinate correctly', () => {
  const tmpDir = makeTempDir();
  const lock1 = createLock(tmpDir, { instanceId: 'inst-1', heartbeatIntervalMs: 100_000 });
  const lock2 = createLock(tmpDir, { instanceId: 'inst-2', heartbeatIntervalMs: 100_000 });

  // Instance 1 acquires
  const r1 = lock1.acquireLock();
  assert.strictEqual(r1.acquired, true);

  // Instance 2 cannot acquire
  const r2 = lock2.acquireLock();
  assert.strictEqual(r2.acquired, false);
  assert.strictEqual(r2.holderId, 'inst-1');

  // Instance 1 releases
  lock1.releaseLock();

  // Instance 2 can now acquire
  const r3 = lock2.acquireLock();
  assert.strictEqual(r3.acquired, true);

  lock2.stopHeartbeat();
  cleanupDir(tmpDir);
});

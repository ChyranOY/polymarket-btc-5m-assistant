import test from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { StateManager } from '../../src/infrastructure/recovery/stateManager.js';
import { TradingState } from '../../src/application/TradingState.js';

// ── Test helpers ────────────────────────────────────────────────────

function makeTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'statemanager-test-'));
}

function createManager(tmpDir) {
  return new StateManager({
    dataDir: tmpDir,
    pidPath: path.join(tmpDir, '.pid'),
    statePath: path.join(tmpDir, 'state.json'),
    writeDebounceMs: 0, // No debounce for tests
    _forceNew: true,
  });
}

function cleanupDir(tmpDir) {
  try { fs.rmSync(tmpDir, { recursive: true, force: true }); } catch { /* ignore */ }
}

// ── PID Lock ────────────────────────────────────────────────────────

test('StateManager: writePidLock creates PID file', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  mgr.writePidLock();
  const pidPath = path.join(tmpDir, '.pid');
  assert.ok(fs.existsSync(pidPath));

  const content = fs.readFileSync(pidPath, 'utf8').trim();
  assert.strictEqual(content, String(process.pid));

  cleanupDir(tmpDir);
});

test('StateManager: removePidLock deletes PID file', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  mgr.writePidLock();
  assert.ok(fs.existsSync(path.join(tmpDir, '.pid')));

  mgr.removePidLock();
  assert.ok(!fs.existsSync(path.join(tmpDir, '.pid')));

  cleanupDir(tmpDir);
});

test('StateManager: checkForCrash returns false when no PID file', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const result = mgr.checkForCrash();
  assert.strictEqual(result.crashed, false);
  assert.strictEqual(result.previousPid, null);

  cleanupDir(tmpDir);
});

test('StateManager: checkForCrash detects crash from stale PID', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  // Write a fake PID that definitely doesn't exist
  fs.writeFileSync(path.join(tmpDir, '.pid'), '999999999', 'utf8');

  const result = mgr.checkForCrash();
  assert.strictEqual(result.crashed, true);
  assert.strictEqual(result.previousPid, 999999999);

  cleanupDir(tmpDir);
});

test('StateManager: checkForCrash detects running process as not crashed', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  // Write our own PID (we're still running)
  fs.writeFileSync(path.join(tmpDir, '.pid'), String(process.pid), 'utf8');

  const result = mgr.checkForCrash();
  assert.strictEqual(result.crashed, false);
  assert.strictEqual(result.previousPid, process.pid);

  cleanupDir(tmpDir);
});

test('StateManager: checkForCrash handles invalid PID file', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  fs.writeFileSync(path.join(tmpDir, '.pid'), 'invalid', 'utf8');

  const result = mgr.checkForCrash();
  assert.strictEqual(result.crashed, true);
  assert.strictEqual(result.previousPid, null);

  cleanupDir(tmpDir);
});

// ── State Persistence ───────────────────────────────────────────────

test('StateManager: persistState writes state file', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const state = new TradingState();
  state.todayRealizedPnl = -25.5;
  state.consecutiveLosses = 3;
  state.killSwitchState.active = true;

  mgr.persistState(state, { immediate: true });

  const statePath = path.join(tmpDir, 'state.json');
  assert.ok(fs.existsSync(statePath));

  const data = JSON.parse(fs.readFileSync(statePath, 'utf8'));
  assert.strictEqual(data.todayRealizedPnl, -25.5);
  assert.strictEqual(data.consecutiveLosses, 3);
  assert.strictEqual(data.killSwitch.active, true);
  assert.ok(data._savedAt);

  cleanupDir(tmpDir);
});

test('StateManager: loadState returns persisted data', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const stateData = {
    killSwitch: { active: true, overrideActive: false, overrideCount: 1, overrideLog: [], lastResetDate: '2026-02-23' },
    todayRealizedPnl: -40,
    consecutiveLosses: 2,
    circuitBreakerTrippedAtMs: 1708700000000,
    hasOpenPosition: true,
    _savedAt: new Date().toISOString(),
  };

  fs.writeFileSync(path.join(tmpDir, 'state.json'), JSON.stringify(stateData), 'utf8');

  const loaded = mgr.loadState();
  assert.ok(loaded);
  assert.strictEqual(loaded.todayRealizedPnl, -40);
  assert.strictEqual(loaded.killSwitch.active, true);
  assert.strictEqual(loaded.consecutiveLosses, 2);

  cleanupDir(tmpDir);
});

test('StateManager: loadState returns null when no file exists', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const loaded = mgr.loadState();
  assert.strictEqual(loaded, null);

  cleanupDir(tmpDir);
});

// ── State Restoration ───────────────────────────────────────────────

test('StateManager: restoreState restores critical fields to TradingState', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const persistedState = {
    killSwitch: {
      active: true,
      overrideActive: true,
      overrideCount: 2,
      overrideLog: [{ timestamp: '2026-02-23T10:00:00Z', count: 1 }],
      lastResetDate: '2026-02-23',
    },
    todayRealizedPnl: -35,
    consecutiveLosses: 4,
    circuitBreakerTrippedAtMs: 1708700000000,
    hasOpenPosition: false,
    _todayKey: '2026-02-23',
  };

  const tradingState = new TradingState();
  const restored = mgr.restoreState(tradingState, persistedState);

  assert.strictEqual(restored, true);
  assert.strictEqual(tradingState.killSwitchState.active, true);
  assert.strictEqual(tradingState.killSwitchState.overrideActive, true);
  assert.strictEqual(tradingState.killSwitchState.overrideCount, 2);
  assert.strictEqual(tradingState.killSwitchState.lastResetDate, '2026-02-23');
  assert.strictEqual(tradingState.todayRealizedPnl, -35);
  assert.strictEqual(tradingState.consecutiveLosses, 4);
  assert.strictEqual(tradingState.circuitBreakerTrippedAtMs, 1708700000000);
  assert.strictEqual(tradingState.hasOpenPosition, false);
  assert.strictEqual(tradingState._todayKey, '2026-02-23');

  cleanupDir(tmpDir);
});

test('StateManager: restoreState returns false when no state available', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const tradingState = new TradingState();
  const restored = mgr.restoreState(tradingState, null);
  assert.strictEqual(restored, false);

  cleanupDir(tmpDir);
});

test('StateManager: restoreState handles partial state gracefully', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  // Only some fields present
  const partialState = {
    todayRealizedPnl: -10,
    // No killSwitch, no consecutiveLosses
  };

  const tradingState = new TradingState();
  const restored = mgr.restoreState(tradingState, partialState);

  assert.strictEqual(restored, true);
  assert.strictEqual(tradingState.todayRealizedPnl, -10);
  // Other fields should remain at defaults
  assert.strictEqual(tradingState.killSwitchState.active, false);
  assert.strictEqual(tradingState.consecutiveLosses, 0);

  cleanupDir(tmpDir);
});

// ── Startup / Shutdown ──────────────────────────────────────────────

test('StateManager: startup detects crash and loads state', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  // Simulate a crashed previous instance
  fs.writeFileSync(path.join(tmpDir, '.pid'), '999999999', 'utf8');
  fs.writeFileSync(
    path.join(tmpDir, 'state.json'),
    JSON.stringify({ todayRealizedPnl: -20, killSwitch: { active: true } }),
    'utf8',
  );

  const result = mgr.startup();
  assert.strictEqual(result.crashed, true);
  assert.strictEqual(result.previousPid, 999999999);
  assert.ok(result.restoredState);
  assert.strictEqual(result.restoredState.todayRealizedPnl, -20);

  // Should have written new PID
  const newPid = fs.readFileSync(path.join(tmpDir, '.pid'), 'utf8').trim();
  assert.strictEqual(newPid, String(process.pid));

  cleanupDir(tmpDir);
});

test('StateManager: startup returns no crash for clean start', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const result = mgr.startup();
  assert.strictEqual(result.crashed, false);
  assert.strictEqual(result.restoredState, null);

  cleanupDir(tmpDir);
});

test('StateManager: shutdown persists state and removes PID', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  mgr.writePidLock();
  assert.ok(fs.existsSync(path.join(tmpDir, '.pid')));

  const state = new TradingState();
  state.todayRealizedPnl = -15;

  mgr.shutdown(state);

  // State should be persisted
  assert.ok(fs.existsSync(path.join(tmpDir, 'state.json')));
  const data = JSON.parse(fs.readFileSync(path.join(tmpDir, 'state.json'), 'utf8'));
  assert.strictEqual(data.todayRealizedPnl, -15);

  // PID should be removed
  assert.ok(!fs.existsSync(path.join(tmpDir, '.pid')));

  cleanupDir(tmpDir);
});

// ── Clear state ─────────────────────────────────────────────────────

test('StateManager: clearState removes state file', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  const state = new TradingState();
  mgr.persistState(state, { immediate: true });
  assert.ok(fs.existsSync(path.join(tmpDir, 'state.json')));

  mgr.clearState();
  assert.ok(!fs.existsSync(path.join(tmpDir, 'state.json')));

  cleanupDir(tmpDir);
});

// ── Roundtrip ───────────────────────────────────────────────────────

test('StateManager: full persist-restore roundtrip preserves critical state', () => {
  const tmpDir = makeTempDir();
  const mgr = createManager(tmpDir);

  // Create state with values
  const original = new TradingState();
  original.todayRealizedPnl = -42.5;
  original.consecutiveLosses = 3;
  original.circuitBreakerTrippedAtMs = Date.now();
  original.hasOpenPosition = true;
  original.killSwitchState = {
    active: true,
    overrideActive: true,
    overrideCount: 2,
    overrideLog: [{ timestamp: '2026-02-23T10:00:00Z', count: 1 }],
    lastResetDate: '2026-02-23',
  };

  // Persist
  mgr.persistState(original, { immediate: true });

  // Load and restore into fresh state
  const loaded = mgr.loadState();
  const restored = new TradingState();
  mgr.restoreState(restored, loaded);

  // Verify
  assert.strictEqual(restored.todayRealizedPnl, -42.5);
  assert.strictEqual(restored.consecutiveLosses, 3);
  assert.strictEqual(restored.circuitBreakerTrippedAtMs, original.circuitBreakerTrippedAtMs);
  assert.strictEqual(restored.hasOpenPosition, true);
  assert.strictEqual(restored.killSwitchState.active, true);
  assert.strictEqual(restored.killSwitchState.overrideActive, true);
  assert.strictEqual(restored.killSwitchState.overrideCount, 2);
  assert.strictEqual(restored.killSwitchState.lastResetDate, '2026-02-23');

  cleanupDir(tmpDir);
});

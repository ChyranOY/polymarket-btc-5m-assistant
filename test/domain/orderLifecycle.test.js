import test from 'node:test';
import assert from 'node:assert';
import { OrderLifecycle, LIFECYCLE_STATES, TRANSITIONS } from '../../src/domain/orderLifecycle.js';

test('orderLifecycle: constructor sets SUBMITTED state with timestamp', () => {
  const before = Date.now();
  const lc = new OrderLifecycle('ord1', { tokenID: 'tok1', side: 'BUY', size: 100 });
  const after = Date.now();

  assert.strictEqual(lc.orderId, 'ord1');
  assert.strictEqual(lc.state, LIFECYCLE_STATES.SUBMITTED);
  assert.ok(lc.timestamps.SUBMITTED >= before && lc.timestamps.SUBMITTED <= after);
  assert.strictEqual(lc.meta.tokenID, 'tok1');
  assert.strictEqual(lc.fillSize, 0);
  assert.strictEqual(lc.fillPrice, 0);
  assert.strictEqual(lc.requestedSize, 100);
});

test('orderLifecycle: valid transitions succeed', () => {
  const lc = new OrderLifecycle('ord1', {});

  assert.strictEqual(lc.transition(LIFECYCLE_STATES.PENDING), true);
  assert.strictEqual(lc.state, LIFECYCLE_STATES.PENDING);
  assert.ok(lc.timestamps.PENDING);

  assert.strictEqual(lc.transition(LIFECYCLE_STATES.FILLED), true);
  assert.strictEqual(lc.state, LIFECYCLE_STATES.FILLED);

  assert.strictEqual(lc.transition(LIFECYCLE_STATES.MONITORING), true);
  assert.strictEqual(lc.state, LIFECYCLE_STATES.MONITORING);

  assert.strictEqual(lc.transition(LIFECYCLE_STATES.EXITED), true);
  assert.strictEqual(lc.state, LIFECYCLE_STATES.EXITED);
});

test('orderLifecycle: invalid transitions fail and preserve state', () => {
  const lc = new OrderLifecycle('ord1', {});

  // SUBMITTED cannot go directly to MONITORING
  assert.strictEqual(lc.transition(LIFECYCLE_STATES.MONITORING), false);
  assert.strictEqual(lc.state, LIFECYCLE_STATES.SUBMITTED);

  // SUBMITTED cannot go to EXITED
  assert.strictEqual(lc.transition(LIFECYCLE_STATES.EXITED), false);
  assert.strictEqual(lc.state, LIFECYCLE_STATES.SUBMITTED);
});

test('orderLifecycle: terminal states reject all transitions', () => {
  // EXITED
  const lc1 = new OrderLifecycle('ord1', {});
  lc1.transition(LIFECYCLE_STATES.PENDING);
  lc1.transition(LIFECYCLE_STATES.FILLED);
  lc1.transition(LIFECYCLE_STATES.MONITORING);
  lc1.transition(LIFECYCLE_STATES.EXITED);
  assert.strictEqual(lc1.transition(LIFECYCLE_STATES.MONITORING), false);
  assert.strictEqual(lc1.state, LIFECYCLE_STATES.EXITED);

  // TIMED_OUT
  const lc2 = new OrderLifecycle('ord2', {});
  lc2.transition(LIFECYCLE_STATES.TIMED_OUT);
  assert.strictEqual(lc2.transition(LIFECYCLE_STATES.PENDING), false);
  assert.strictEqual(lc2.state, LIFECYCLE_STATES.TIMED_OUT);

  // CANCELLED
  const lc3 = new OrderLifecycle('ord3', {});
  lc3.transition(LIFECYCLE_STATES.CANCELLED);
  assert.strictEqual(lc3.transition(LIFECYCLE_STATES.SUBMITTED), false);
  assert.strictEqual(lc3.state, LIFECYCLE_STATES.CANCELLED);

  // FAILED
  const lc4 = new OrderLifecycle('ord4', {});
  lc4.transition(LIFECYCLE_STATES.FAILED);
  assert.strictEqual(lc4.transition(LIFECYCLE_STATES.PENDING), false);
  assert.strictEqual(lc4.state, LIFECYCLE_STATES.FAILED);
});

test('orderLifecycle: isTimedOut returns true after timeout threshold', () => {
  const lc = new OrderLifecycle('ord1', {});
  // Override timestamp to simulate elapsed time
  lc.timestamps[LIFECYCLE_STATES.SUBMITTED] = Date.now() - 31_000;

  assert.strictEqual(lc.isTimedOut(30_000), true);
});

test('orderLifecycle: isTimedOut returns false before timeout threshold', () => {
  const lc = new OrderLifecycle('ord1', {});
  // Just created — well within timeout
  assert.strictEqual(lc.isTimedOut(30_000), false);
});

test('orderLifecycle: isTimedOut returns false for non-pending states', () => {
  const lc = new OrderLifecycle('ord1', {});
  lc.transition(LIFECYCLE_STATES.PENDING);
  lc.transition(LIFECYCLE_STATES.FILLED);
  lc.transition(LIFECYCLE_STATES.MONITORING);
  // Override to simulate old timestamp
  lc.timestamps[LIFECYCLE_STATES.SUBMITTED] = Date.now() - 60_000;

  assert.strictEqual(lc.isTimedOut(30_000), false);
});

test('orderLifecycle: isTerminal identifies terminal states', () => {
  const terminals = [LIFECYCLE_STATES.EXITED, LIFECYCLE_STATES.TIMED_OUT, LIFECYCLE_STATES.CANCELLED, LIFECYCLE_STATES.FAILED];
  const nonTerminals = [LIFECYCLE_STATES.SUBMITTED, LIFECYCLE_STATES.PENDING, LIFECYCLE_STATES.FILLED, LIFECYCLE_STATES.PARTIAL_FILL, LIFECYCLE_STATES.MONITORING];

  for (const state of terminals) {
    const lc = new OrderLifecycle('ord1', {});
    lc.state = state; // Direct set for testing
    assert.strictEqual(lc.isTerminal(), true, `${state} should be terminal`);
  }

  for (const state of nonTerminals) {
    const lc = new OrderLifecycle('ord1', {});
    lc.state = state;
    assert.strictEqual(lc.isTerminal(), false, `${state} should not be terminal`);
  }
});

test('orderLifecycle: recordFill stores fill data', () => {
  const lc = new OrderLifecycle('ord1', { size: 100 });
  lc.recordFill(100, 0.55);

  assert.strictEqual(lc.fillSize, 100);
  assert.strictEqual(lc.fillPrice, 0.55);
  assert.strictEqual(lc.fillRatio, 1); // 100/100 = 1
});

test('orderLifecycle: recordPartialFill stores partial data with ratio', () => {
  const lc = new OrderLifecycle('ord1', { size: 100 });
  lc.recordPartialFill(30, 0.55, 50);

  assert.strictEqual(lc.fillSize, 30);
  assert.strictEqual(lc.fillPrice, 0.55);
  assert.strictEqual(lc.requestedSize, 50);
  assert.strictEqual(lc.fillRatio, 0.6); // 30/50
});

test('orderLifecycle: getView returns snapshot', () => {
  const lc = new OrderLifecycle('ord1', { tokenID: 'tok1', side: 'BUY', price: 0.5, size: 100, extra: { marketSlug: 'btc-5m' } });
  lc.transition(LIFECYCLE_STATES.PENDING);
  lc.recordFill(100, 0.52);

  const view = lc.getView();

  assert.strictEqual(view.orderId, 'ord1');
  assert.strictEqual(view.state, LIFECYCLE_STATES.PENDING);
  assert.strictEqual(view.tokenID, 'tok1');
  assert.strictEqual(view.side, 'BUY');
  assert.strictEqual(view.fillSize, 100);
  assert.strictEqual(view.fillPrice, 0.52);
  assert.ok(view.timestamps.SUBMITTED);
  assert.ok(view.timestamps.PENDING);
  assert.deepStrictEqual(view.extra, { marketSlug: 'btc-5m' });
});

test('orderLifecycle: partial fill path SUBMITTED -> PENDING -> PARTIAL_FILL -> MONITORING -> EXITED', () => {
  const lc = new OrderLifecycle('ord1', { size: 100 });

  assert.strictEqual(lc.transition(LIFECYCLE_STATES.PENDING), true);
  assert.strictEqual(lc.transition(LIFECYCLE_STATES.PARTIAL_FILL), true);
  lc.recordPartialFill(60, 0.55, 100);
  assert.strictEqual(lc.transition(LIFECYCLE_STATES.MONITORING), true);
  assert.strictEqual(lc.transition(LIFECYCLE_STATES.EXITED), true);

  assert.strictEqual(lc.fillRatio, 0.6);
  assert.strictEqual(lc.isTerminal(), true);
});

test('orderLifecycle: TRANSITIONS map covers all states', () => {
  const allStates = Object.values(LIFECYCLE_STATES);
  for (const state of allStates) {
    assert.ok(state in TRANSITIONS, `Missing transitions for state: ${state}`);
    assert.ok(Array.isArray(TRANSITIONS[state]), `Transitions for ${state} should be array`);
  }
});

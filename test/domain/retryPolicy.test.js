import test from 'node:test';
import assert from 'node:assert';
import { isRetryableError, withOrderRetry, createFailureEvent } from '../../src/domain/retryPolicy.js';

// ── isRetryableError tests ──────────────────────────────────────────

test('retryPolicy: ECONNRESET is retryable', () => {
  assert.strictEqual(isRetryableError({ code: 'ECONNRESET' }), true);
});

test('retryPolicy: ETIMEDOUT is retryable', () => {
  assert.strictEqual(isRetryableError({ code: 'ETIMEDOUT' }), true);
});

test('retryPolicy: ENOTFOUND is retryable', () => {
  assert.strictEqual(isRetryableError({ code: 'ENOTFOUND' }), true);
});

test('retryPolicy: AbortError is retryable', () => {
  assert.strictEqual(isRetryableError({ name: 'AbortError' }), true);
});

test('retryPolicy: 500 server error is retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 500 } }), true);
});

test('retryPolicy: 502 server error is retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 502 } }), true);
});

test('retryPolicy: 503 server error is retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 503 } }), true);
});

test('retryPolicy: 429 rate limit is retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 429 } }), true);
});

test('retryPolicy: 401 auth failure is NOT retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 401 } }), false);
});

test('retryPolicy: 403 forbidden is NOT retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 403 } }), false);
});

test('retryPolicy: 400 bad request is NOT retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 400 } }), false);
});

test('retryPolicy: 422 insufficient funds is NOT retryable', () => {
  assert.strictEqual(isRetryableError({ response: { status: 422 } }), false);
});

test('retryPolicy: "fetch failed" message is retryable', () => {
  assert.strictEqual(isRetryableError({ message: 'fetch failed: network error' }), true);
});

test('retryPolicy: null/undefined error is not retryable', () => {
  assert.strictEqual(isRetryableError(null), false);
  assert.strictEqual(isRetryableError(undefined), false);
});

test('retryPolicy: unknown error is not retryable (conservative)', () => {
  assert.strictEqual(isRetryableError({ message: 'some unknown error' }), false);
});

// ── withOrderRetry tests ────────────────────────────────────────────

test('retryPolicy: withOrderRetry succeeds on first attempt', async () => {
  let callCount = 0;
  const result = await withOrderRetry(async () => {
    callCount++;
    return { ok: true };
  }, { delays: [1, 1, 1] }); // Tiny delays for fast tests

  assert.strictEqual(callCount, 1);
  assert.deepStrictEqual(result, { ok: true });
});

test('retryPolicy: withOrderRetry retries on retryable error and succeeds', async () => {
  let callCount = 0;
  const result = await withOrderRetry(async () => {
    callCount++;
    if (callCount < 3) throw { code: 'ECONNRESET', message: 'connection reset' };
    return { ok: true };
  }, { maxAttempts: 3, delays: [1, 1, 1] });

  assert.strictEqual(callCount, 3);
  assert.deepStrictEqual(result, { ok: true });
});

test('retryPolicy: withOrderRetry exhausts all attempts on persistent retryable error', async () => {
  let callCount = 0;
  await assert.rejects(
    () => withOrderRetry(async () => {
      callCount++;
      throw { code: 'ECONNRESET', message: 'connection reset' };
    }, { maxAttempts: 3, delays: [1, 1, 1] }),
    (err) => err.code === 'ECONNRESET',
  );

  assert.strictEqual(callCount, 3);
});

test('retryPolicy: withOrderRetry does NOT retry on fatal error', async () => {
  let callCount = 0;
  await assert.rejects(
    () => withOrderRetry(async () => {
      callCount++;
      throw { response: { status: 401 }, message: 'Unauthorized' };
    }, { maxAttempts: 3, delays: [1, 1, 1] }),
    (err) => err.response?.status === 401,
  );

  assert.strictEqual(callCount, 1); // Only one attempt — no retry
});

test('retryPolicy: withOrderRetry respects maxAttempts option', async () => {
  let callCount = 0;
  await assert.rejects(
    () => withOrderRetry(async () => {
      callCount++;
      throw { code: 'ETIMEDOUT', message: 'timed out' };
    }, { maxAttempts: 2, delays: [1, 1] }),
  );

  assert.strictEqual(callCount, 2);
});

// ── createFailureEvent tests ────────────────────────────────────────

test('retryPolicy: createFailureEvent returns correct structure', () => {
  const err = { message: 'connection reset', code: 'ECONNRESET' };
  const event = createFailureEvent('ord123', err, 3);

  assert.strictEqual(event.type, 'ORDER_FAILED');
  assert.strictEqual(event.orderId, 'ord123');
  assert.strictEqual(event.error.message, 'connection reset');
  assert.strictEqual(event.error.code, 'ECONNRESET');
  assert.strictEqual(event.error.retryable, true);
  assert.strictEqual(event.retryCount, 3);
  assert.strictEqual(event.severity, 'critical');
  assert.strictEqual(event.category, 'order_execution');
  assert.ok(event.timestamp); // ISO string
});

test('retryPolicy: createFailureEvent handles HTTP error', () => {
  const err = { message: 'Bad Request', response: { status: 400 } };
  const event = createFailureEvent('ord456', err, 1);

  assert.strictEqual(event.error.status, 400);
  assert.strictEqual(event.error.retryable, false);
});

test('retryPolicy: createFailureEvent handles null error', () => {
  const event = createFailureEvent(null, null, 0);

  assert.strictEqual(event.orderId, null);
  assert.strictEqual(event.error.message, 'Unknown error');
  assert.strictEqual(event.retryCount, 0);
});

/**
 * @file Retry policy and error classification — pure domain logic.
 *
 * Classifies CLOB errors as retryable vs fatal. Provides an async retry
 * wrapper with exponential backoff. Creates structured failure events
 * for Phase 4 webhook consumption.
 *
 * Pure functions (except withOrderRetry which uses setTimeout for delays).
 */

/**
 * Classify whether an error is retryable.
 *
 * Retryable: network errors, timeouts, 5xx server errors, rate limits (429).
 * Fatal: authentication (401/403), invalid params (400), insufficient funds (422).
 *
 * @param {Error|Object} err
 * @returns {boolean}
 */
export function isRetryableError(err) {
  if (!err) return false;

  // Node.js network-level errors
  const code = err.code || err?.cause?.code;
  if (code === 'ECONNRESET' || code === 'ETIMEDOUT' || code === 'ENOTFOUND' ||
      code === 'ECONNREFUSED' || code === 'EPIPE' || code === 'UND_ERR_CONNECT_TIMEOUT') {
    return true;
  }

  // AbortError from fetch timeout
  if (err.name === 'AbortError' || err.name === 'TimeoutError') {
    return true;
  }

  // HTTP status-based classification
  const status = err.response?.status ?? err.status ?? err?.cause?.status;
  if (typeof status === 'number') {
    // 5xx server errors
    if (status >= 500) return true;
    // Rate limited
    if (status === 429) return true;
    // Client errors are fatal (401, 403, 400, 422)
    if (status === 401 || status === 403 || status === 400 || status === 422) return false;
  }

  // Generic network error patterns in message
  const msg = String(err.message || '').toLowerCase();
  if (msg.includes('fetch failed') || msg.includes('network') || msg.includes('socket hang up') ||
      msg.includes('econnreset') || msg.includes('etimedout')) {
    return true;
  }

  // Default: not retryable (conservative — don't retry unknown errors)
  return false;
}

/**
 * Execute an async function with retry and exponential backoff.
 *
 * @param {Function} fn - Async function to execute
 * @param {Object} [opts]
 * @param {number} [opts.maxAttempts=3] - Maximum attempts
 * @param {number[]} [opts.delays=[1000,2000,4000]] - Delay between attempts (ms)
 * @param {number} [opts.maxDelayMs=30000] - Maximum delay cap (ms)
 * @param {string} [opts.orderId] - Order ID for failure event
 * @returns {Promise<Object>} Result from fn, or { failed: true, error, retryCount }
 */
export async function withOrderRetry(fn, opts = {}) {
  const maxAttempts = opts.maxAttempts ?? 3;
  const delays = opts.delays ?? [1000, 2000, 4000];
  const maxDelayMs = opts.maxDelayMs ?? 30000;

  let lastError = null;

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      const result = await fn();
      return result;
    } catch (err) {
      lastError = err;

      // Fatal errors — fail immediately, no retry
      if (!isRetryableError(err)) {
        throw err;
      }

      // Last attempt — don't delay, just throw
      if (attempt === maxAttempts) {
        throw err;
      }

      // Delay before next attempt
      const delay = Math.min(delays[attempt - 1] ?? delays[delays.length - 1] ?? 4000, maxDelayMs);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }

  // Should not reach here, but safety
  throw lastError;
}

/**
 * Create a structured failure event for Phase 4 webhook consumption.
 *
 * @param {string} orderId
 * @param {Error|Object} error
 * @param {number} retryCount
 * @returns {Object}
 */
export function createFailureEvent(orderId, error, retryCount) {
  return {
    type: 'ORDER_FAILED',
    orderId: orderId || null,
    error: {
      message: error?.message || String(error || 'Unknown error'),
      code: error?.code || null,
      status: error?.response?.status ?? error?.status ?? null,
      retryable: isRetryableError(error),
    },
    retryCount: retryCount ?? 0,
    timestamp: new Date().toISOString(),
    severity: 'critical',
    category: 'order_execution',
  };
}

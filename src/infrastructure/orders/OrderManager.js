/**
 * @file Order lifecycle manager for the Polymarket CLOB.
 *
 * Tracks pending orders in memory using OrderLifecycle instances,
 * provides cancel/list functionality, and reconciles pending orders
 * by polling the CLOB API.
 */

import { getClobClient } from '../../live_trading/clob.js';
import { OrderLifecycle, LIFECYCLE_STATES } from '../../domain/orderLifecycle.js';

export class OrderManager {
  constructor() {
    /** @type {import('@polymarket/clob-client').ClobClient|null} */
    this._client = null;

    /** @type {Map<string, OrderLifecycle>} orderId -> OrderLifecycle */
    this._orders = new Map();

    /** @type {number} */
    this._lastReconcileMs = 0;
  }

  /**
   * Lazy-initialize the CLOB client.
   * @returns {import('@polymarket/clob-client').ClobClient|null}
   */
  _getClient() {
    if (!this._client) {
      try {
        this._client = getClobClient();
      } catch {
        // not available
      }
    }
    return this._client;
  }

  /**
   * Start tracking a new order with a full lifecycle.
   * @param {string} orderId
   * @param {Object} metadata
   * @param {string} metadata.tokenID
   * @param {'BUY'|'SELL'} metadata.side
   * @param {number} metadata.price
   * @param {number} metadata.size
   * @param {Object} [metadata.extra] - Any extra context
   * @returns {OrderLifecycle|null} The created lifecycle, or null if orderId missing
   */
  trackOrder(orderId, metadata) {
    if (!orderId) return null;

    const lifecycle = new OrderLifecycle(orderId, {
      tokenID: metadata.tokenID,
      side: metadata.side,
      price: metadata.price,
      size: metadata.size,
      extra: metadata.extra || null,
    });

    this._orders.set(orderId, lifecycle);
    return lifecycle;
  }

  /**
   * Get an order lifecycle by ID.
   * @param {string} orderId
   * @returns {OrderLifecycle|null}
   */
  getOrder(orderId) {
    return this._orders.get(orderId) || null;
  }

  /**
   * Transition an order to a new state.
   * @param {string} orderId
   * @param {string} newState - One of LIFECYCLE_STATES
   * @returns {boolean} True if transition succeeded
   */
  transitionOrder(orderId, newState) {
    const lifecycle = this._orders.get(orderId);
    if (!lifecycle) return false;
    return lifecycle.transition(newState);
  }

  /**
   * Get all active (non-terminal) orders.
   * @returns {OrderLifecycle[]}
   */
  getActiveOrders() {
    return [...this._orders.values()].filter(o => !o.isTerminal());
  }

  /**
   * Get orders that have exceeded the fill timeout.
   * @param {number} [timeoutMs=30000]
   * @returns {OrderLifecycle[]}
   */
  getTimedOutOrders(timeoutMs = 30_000) {
    return [...this._orders.values()].filter(o => o.isTimedOut(timeoutMs));
  }

  /**
   * Check all orders for timeouts and return the timed-out ones.
   * Does NOT automatically transition them — caller decides what to do.
   * @param {number} [timeoutMs=30000]
   * @returns {OrderLifecycle[]}
   */
  checkTimeouts(timeoutMs = 30_000) {
    return this.getTimedOutOrders(timeoutMs);
  }

  /**
   * Get a view snapshot of a single order for UI/API.
   * @param {string} orderId
   * @returns {Object|null}
   */
  getOrderView(orderId) {
    const lifecycle = this._orders.get(orderId);
    return lifecycle ? lifecycle.getView() : null;
  }

  /**
   * Get view snapshots for all tracked orders.
   * @returns {Object[]}
   */
  getAllOrderViews() {
    return [...this._orders.values()].map(o => o.getView());
  }

  /**
   * Cancel a specific order.
   * @param {string} orderId
   * @returns {Promise<{ cancelled: boolean, error?: string }>}
   */
  async cancelOrder(orderId) {
    const client = this._getClient();
    if (!client) {
      return { cancelled: false, error: 'CLOB client not available' };
    }

    try {
      await client.cancelOrder({ orderID: orderId });

      const lifecycle = this._orders.get(orderId);
      if (lifecycle) {
        lifecycle.transition(LIFECYCLE_STATES.CANCELLED);
      }

      return { cancelled: true };
    } catch (e) {
      return { cancelled: false, error: e?.message || String(e) };
    }
  }

  /**
   * Cancel all open orders.
   * @returns {Promise<{ cancelled: boolean, result?: any, error?: string }>}
   */
  async cancelAllOrders() {
    const client = this._getClient();
    if (!client) {
      return { cancelled: false, error: 'CLOB client not available' };
    }

    try {
      const result = await client.cancelAll();

      // Transition all active orders to CANCELLED
      for (const lifecycle of this._orders.values()) {
        if (!lifecycle.isTerminal()) {
          lifecycle.transition(LIFECYCLE_STATES.CANCELLED);
        }
      }

      return { cancelled: true, result };
    } catch (e) {
      return { cancelled: false, error: e?.message || String(e) };
    }
  }

  /**
   * Reconcile pending orders by polling the CLOB API.
   * Checks the status of each tracked pending/open order and transitions lifecycle.
   *
   * @param {Object} [opts]
   * @param {number} [opts.minIntervalMs] - Min time between reconciliations (default 5s)
   * @returns {Promise<{ reconciled: number, filled: string[], cancelled: string[] }>}
   */
  async reconcilePendingOrders(opts = {}) {
    const minInterval = opts.minIntervalMs ?? 5_000;
    const now = Date.now();

    if (now - this._lastReconcileMs < minInterval) {
      return { reconciled: 0, filled: [], cancelled: [] };
    }
    this._lastReconcileMs = now;

    const client = this._getClient();
    if (!client) {
      return { reconciled: 0, filled: [], cancelled: [] };
    }

    const filled = [];
    const cancelled = [];
    let reconciled = 0;

    for (const [orderId, lifecycle] of this._orders) {
      if (lifecycle.isTerminal()) continue;
      // Only reconcile SUBMITTED and PENDING orders
      if (lifecycle.state !== LIFECYCLE_STATES.SUBMITTED &&
          lifecycle.state !== LIFECYCLE_STATES.PENDING) continue;

      try {
        const apiOrder = await client.getOrder(orderId);
        reconciled++;

        if (!apiOrder) {
          lifecycle.transition(LIFECYCLE_STATES.FAILED);
          lifecycle.error = 'Order not found via API';
          continue;
        }

        const apiStatus = String(apiOrder.status || apiOrder.order_status || '').toLowerCase();

        if (apiStatus === 'filled' || apiStatus === 'matched') {
          // Transition through PENDING -> FILLED if still SUBMITTED
          if (lifecycle.state === LIFECYCLE_STATES.SUBMITTED) {
            lifecycle.transition(LIFECYCLE_STATES.PENDING);
          }
          lifecycle.transition(LIFECYCLE_STATES.FILLED);

          // Record fill details if available
          const fillSize = Number(apiOrder.size_matched || apiOrder.fillSize || lifecycle.meta.size || 0);
          const fillPrice = Number(apiOrder.price || lifecycle.meta.price || 0);
          lifecycle.recordFill(fillSize, fillPrice);

          filled.push(orderId);
        } else if (apiStatus === 'cancelled' || apiStatus === 'canceled') {
          lifecycle.transition(LIFECYCLE_STATES.CANCELLED);
          cancelled.push(orderId);
        } else if (apiStatus === 'live' || apiStatus === 'open') {
          // Transition to PENDING if still SUBMITTED
          if (lifecycle.state === LIFECYCLE_STATES.SUBMITTED) {
            lifecycle.transition(LIFECYCLE_STATES.PENDING);
          }
        }
      } catch {
        // Skip — will retry next reconciliation
      }
    }

    return { reconciled, filled, cancelled };
  }

  /**
   * Get all tracked orders, optionally filtered by status.
   * Returns TrackedOrder-shaped objects for backward compatibility.
   * @param {Object} [opts]
   * @param {string} [opts.status] - Filter by legacy status name ('pending'|'open'|'filled'|'cancelled')
   * @returns {Object[]}
   */
  getPendingOrders(opts = {}) {
    const orders = this.getAllOrderViews().map(v => ({
      orderId: v.orderId,
      tokenID: v.tokenID,
      side: v.side,
      price: v.price,
      size: v.size,
      status: this._lifecycleStateToLegacy(v.state),
      createdAt: v.timestamps?.SUBMITTED
        ? new Date(v.timestamps.SUBMITTED).toISOString()
        : new Date().toISOString(),
      updatedAt: null,
      metadata: v.extra,
    }));

    if (opts.status) {
      return orders.filter(o => o.status === opts.status);
    }
    return orders;
  }

  /**
   * Get a snapshot for the API.
   * @returns {{ total: number, pending: number, open: number, filled: number, cancelled: number, orders: Object[] }}
   */
  getSnapshot() {
    const orders = this.getPendingOrders();
    return {
      total: orders.length,
      pending: orders.filter(o => o.status === 'pending').length,
      open: orders.filter(o => o.status === 'open').length,
      filled: orders.filter(o => o.status === 'filled').length,
      cancelled: orders.filter(o => o.status === 'cancelled').length,
      orders,
    };
  }

  /**
   * Clean up old orders (e.g., terminal older than N minutes).
   * @param {number} [maxAgeMs=30*60_000] - Max age for completed orders (default 30 min)
   */
  pruneOldOrders(maxAgeMs = 30 * 60_000) {
    const cutoff = Date.now() - maxAgeMs;
    for (const [orderId, lifecycle] of this._orders) {
      if (lifecycle.isTerminal()) {
        const submittedAt = lifecycle.timestamps[LIFECYCLE_STATES.SUBMITTED] || 0;
        if (submittedAt < cutoff) {
          this._orders.delete(orderId);
        }
      }
    }
  }

  /**
   * Map lifecycle state to legacy status string for backward compatibility.
   * @param {string} state - LIFECYCLE_STATES value
   * @returns {string}
   */
  _lifecycleStateToLegacy(state) {
    switch (state) {
      case LIFECYCLE_STATES.SUBMITTED:
        return 'pending';
      case LIFECYCLE_STATES.PENDING:
        return 'open';
      case LIFECYCLE_STATES.FILLED:
      case LIFECYCLE_STATES.PARTIAL_FILL:
      case LIFECYCLE_STATES.MONITORING:
      case LIFECYCLE_STATES.EXITED:
        return 'filled';
      case LIFECYCLE_STATES.CANCELLED:
        return 'cancelled';
      case LIFECYCLE_STATES.TIMED_OUT:
        return 'cancelled';
      case LIFECYCLE_STATES.FAILED:
        return 'cancelled';
      default:
        return 'unknown';
    }
  }
}

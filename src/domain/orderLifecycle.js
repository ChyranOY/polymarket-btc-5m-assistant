/**
 * @file Order lifecycle state machine — pure domain logic.
 *
 * Tracks an order through its lifecycle: SUBMITTED → PENDING → FILLED → MONITORING → EXITED.
 * Also handles partial fills, timeouts, and cancellations.
 *
 * Pure class — no I/O, no side effects, no external dependencies.
 */

export const LIFECYCLE_STATES = {
  SUBMITTED: 'SUBMITTED',
  PENDING: 'PENDING',
  FILLED: 'FILLED',
  PARTIAL_FILL: 'PARTIAL_FILL',
  MONITORING: 'MONITORING',
  EXITED: 'EXITED',
  TIMED_OUT: 'TIMED_OUT',
  CANCELLED: 'CANCELLED',
  FAILED: 'FAILED',
};

/**
 * Valid state transitions. Each key maps to an array of states it can transition to.
 */
export const TRANSITIONS = {
  [LIFECYCLE_STATES.SUBMITTED]: [
    LIFECYCLE_STATES.PENDING,
    LIFECYCLE_STATES.TIMED_OUT,
    LIFECYCLE_STATES.FAILED,
    LIFECYCLE_STATES.CANCELLED,
  ],
  [LIFECYCLE_STATES.PENDING]: [
    LIFECYCLE_STATES.FILLED,
    LIFECYCLE_STATES.PARTIAL_FILL,
    LIFECYCLE_STATES.TIMED_OUT,
    LIFECYCLE_STATES.FAILED,
    LIFECYCLE_STATES.CANCELLED,
  ],
  [LIFECYCLE_STATES.FILLED]: [
    LIFECYCLE_STATES.MONITORING,
  ],
  [LIFECYCLE_STATES.PARTIAL_FILL]: [
    LIFECYCLE_STATES.MONITORING,
  ],
  [LIFECYCLE_STATES.MONITORING]: [
    LIFECYCLE_STATES.EXITED,
    LIFECYCLE_STATES.FAILED,
  ],
  // Terminal states — no transitions out
  [LIFECYCLE_STATES.EXITED]: [],
  [LIFECYCLE_STATES.TIMED_OUT]: [],
  [LIFECYCLE_STATES.CANCELLED]: [],
  [LIFECYCLE_STATES.FAILED]: [],
};

const TERMINAL_STATES = new Set([
  LIFECYCLE_STATES.EXITED,
  LIFECYCLE_STATES.TIMED_OUT,
  LIFECYCLE_STATES.CANCELLED,
  LIFECYCLE_STATES.FAILED,
]);

export class OrderLifecycle {
  /**
   * @param {string} orderId
   * @param {Object} meta - Metadata: tokenID, side, price, size, extra
   */
  constructor(orderId, meta = {}) {
    this.orderId = orderId;
    this.state = LIFECYCLE_STATES.SUBMITTED;
    this.meta = meta;
    this.timestamps = { [LIFECYCLE_STATES.SUBMITTED]: Date.now() };

    /** @type {number} Actual filled size (shares) */
    this.fillSize = 0;
    /** @type {number} Actual fill price */
    this.fillPrice = 0;
    /** @type {number} Originally requested size */
    this.requestedSize = meta.size ?? 0;
    /** @type {number} Fill ratio (0..1) */
    this.fillRatio = 0;
    /** @type {string|null} Error message if failed */
    this.error = null;
  }

  /**
   * Attempt to transition to a new state.
   * @param {string} newState - One of LIFECYCLE_STATES
   * @returns {boolean} True if transition was valid and applied
   */
  transition(newState) {
    const validNextStates = TRANSITIONS[this.state];
    if (!validNextStates || !validNextStates.includes(newState)) {
      return false;
    }
    this.state = newState;
    this.timestamps[newState] = Date.now();
    return true;
  }

  /**
   * Record a full fill.
   * @param {number} size - Filled size (shares)
   * @param {number} price - Fill price
   */
  recordFill(size, price) {
    this.fillSize = size;
    this.fillPrice = price;
    this.fillRatio = this.requestedSize > 0 ? size / this.requestedSize : 1;
  }

  /**
   * Record a partial fill.
   * @param {number} filledSize - Actually filled shares
   * @param {number} price - Fill price
   * @param {number} requestedSize - Originally requested shares
   */
  recordPartialFill(filledSize, price, requestedSize) {
    this.fillSize = filledSize;
    this.fillPrice = price;
    this.requestedSize = requestedSize;
    this.fillRatio = requestedSize > 0 ? filledSize / requestedSize : 0;
  }

  /**
   * Check if this order has exceeded the fill timeout.
   * @param {number} [timeoutMs=30000] - Timeout in milliseconds
   * @returns {boolean}
   */
  isTimedOut(timeoutMs = 30_000) {
    if (this.state === LIFECYCLE_STATES.SUBMITTED || this.state === LIFECYCLE_STATES.PENDING) {
      return Date.now() - this.timestamps[LIFECYCLE_STATES.SUBMITTED] > timeoutMs;
    }
    return false;
  }

  /**
   * Check if this order is in a terminal state.
   * @returns {boolean}
   */
  isTerminal() {
    return TERMINAL_STATES.has(this.state);
  }

  /**
   * Get a snapshot view of this order for UI/API consumption.
   * @returns {Object}
   */
  getView() {
    return {
      orderId: this.orderId,
      state: this.state,
      tokenID: this.meta.tokenID ?? null,
      side: this.meta.side ?? null,
      price: this.meta.price ?? 0,
      size: this.meta.size ?? 0,
      fillSize: this.fillSize,
      fillPrice: this.fillPrice,
      fillRatio: this.fillRatio,
      requestedSize: this.requestedSize,
      timestamps: { ...this.timestamps },
      error: this.error,
      extra: this.meta.extra ?? null,
    };
  }
}

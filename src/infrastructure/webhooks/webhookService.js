/**
 * @file Webhook alerting service — fire-and-forget delivery to Slack/Discord.
 *
 * Critical events only:
 *   - Kill-switch activation
 *   - Circuit breaker trip
 *   - ORDER_FAILED (after retries exhausted)
 *   - Process crash
 *
 * Architecture: Infrastructure layer (I/O — outbound HTTP).
 *
 * Configuration:
 *   WEBHOOK_URL   — Slack or Discord incoming webhook URL
 *   WEBHOOK_TYPE  — "slack" (default) or "discord"
 *
 * Delivery: fire-and-forget. Attempt delivery once, log failure, never retry.
 * Webhooks are notifications, not critical path. Never blocks the trading loop.
 */

// ── Event types ────────────────────────────────────────────────────
export const WEBHOOK_EVENTS = {
  KILL_SWITCH: 'KILL_SWITCH',
  CIRCUIT_BREAKER: 'CIRCUIT_BREAKER',
  ORDER_FAILED: 'ORDER_FAILED',
  PROCESS_CRASH: 'PROCESS_CRASH',
  RECONCILIATION_DISCREPANCY: 'RECONCILIATION_DISCREPANCY',
};

// ── Severity levels ────────────────────────────────────────────────
const SEVERITY = {
  critical: { emoji: '🚨', color: '#FF0000', label: 'CRITICAL' },
  warning: { emoji: '⚠️', color: '#FFA500', label: 'WARNING' },
  info: { emoji: 'ℹ️', color: '#2196F3', label: 'INFO' },
};

// ── Singleton ──────────────────────────────────────────────────────
let _instance = null;

/**
 * Get the singleton WebhookService instance.
 * @param {Object} [opts] - Override config for testing
 * @returns {WebhookService}
 */
export function getWebhookService(opts = {}) {
  if (_instance && !opts._forceNew) return _instance;
  const svc = new WebhookService(opts);
  if (!opts._forceNew) _instance = svc;
  return svc;
}

/**
 * Reset singleton (for testing).
 */
export function resetWebhookService() {
  _instance = null;
}

// ── Slack formatter ────────────────────────────────────────────────

/**
 * Format an alert for Slack incoming webhook.
 * @param {Object} alert
 * @returns {Object} Slack payload
 */
export function formatForSlack(alert) {
  const sev = SEVERITY[alert.severity] || SEVERITY.info;

  const fields = [];
  if (alert.details) {
    for (const [key, value] of Object.entries(alert.details)) {
      if (value !== null && value !== undefined) {
        fields.push({
          type: 'mrkdwn',
          text: `*${key}:* ${value}`,
        });
      }
    }
  }

  const blocks = [
    {
      type: 'header',
      text: {
        type: 'plain_text',
        text: `${sev.emoji} ${sev.label}: ${alert.title}`,
        emoji: true,
      },
    },
    {
      type: 'section',
      text: {
        type: 'mrkdwn',
        text: alert.message || 'No additional details.',
      },
    },
  ];

  if (fields.length > 0) {
    blocks.push({
      type: 'section',
      fields: fields.slice(0, 10), // Slack limit: 10 fields per section
    });
  }

  blocks.push({
    type: 'context',
    elements: [
      {
        type: 'mrkdwn',
        text: `Polymarket BTC 5m Bot | ${new Date().toISOString()}`,
      },
    ],
  });

  return { blocks };
}

// ── Discord formatter ──────────────────────────────────────────────

/**
 * Format an alert for Discord webhook.
 * Discord accepts Slack-compatible format, but embeds look better.
 * @param {Object} alert
 * @returns {Object} Discord payload
 */
export function formatForDiscord(alert) {
  const sev = SEVERITY[alert.severity] || SEVERITY.info;

  const fields = [];
  if (alert.details) {
    for (const [key, value] of Object.entries(alert.details)) {
      if (value !== null && value !== undefined) {
        fields.push({
          name: key,
          value: String(value),
          inline: true,
        });
      }
    }
  }

  return {
    embeds: [
      {
        title: `${sev.emoji} ${sev.label}: ${alert.title}`,
        description: alert.message || 'No additional details.',
        color: parseInt(sev.color.replace('#', ''), 16),
        fields: fields.slice(0, 25), // Discord limit: 25 fields
        timestamp: new Date().toISOString(),
        footer: {
          text: 'Polymarket BTC 5m Bot',
        },
      },
    ],
  };
}

// ── WebhookService class ───────────────────────────────────────────

export class WebhookService {
  /**
   * @param {Object} [opts]
   * @param {string} [opts.url] - Webhook URL (overrides env)
   * @param {string} [opts.type] - 'slack' or 'discord' (overrides env)
   * @param {Function} [opts.fetchFn] - Custom fetch function (for testing)
   */
  constructor(opts = {}) {
    this.url = opts.url || process.env.WEBHOOK_URL || null;
    this.type = (opts.type || process.env.WEBHOOK_TYPE || 'slack').toLowerCase();
    this._fetchFn = opts.fetchFn || globalThis.fetch;

    // Delivery stats
    this._stats = {
      sent: 0,
      failed: 0,
      lastSentAt: null,
      lastError: null,
    };

    // Deduplication: prevent spamming the same event type within cooldown
    this._lastEventByType = new Map();
    this._cooldownMs = opts.cooldownMs || 60_000; // 1 minute default
  }

  /**
   * Check if webhooks are configured.
   * @returns {boolean}
   */
  isConfigured() {
    return Boolean(this.url);
  }

  /**
   * Send a critical alert. Fire-and-forget — never throws.
   *
   * @param {Object} alert
   * @param {string} alert.event - Event type (from WEBHOOK_EVENTS)
   * @param {string} alert.title - Short title
   * @param {string} alert.message - Detail message
   * @param {'critical'|'warning'|'info'} alert.severity
   * @param {Object} [alert.details] - Key-value pairs for structured data
   */
  async send(alert) {
    if (!this.isConfigured()) return;

    // Deduplication check
    const eventType = alert.event || 'unknown';
    const lastSent = this._lastEventByType.get(eventType);
    const now = Date.now();
    if (lastSent && (now - lastSent < this._cooldownMs)) {
      return; // Skip duplicate within cooldown
    }

    try {
      const payload = this.type === 'discord'
        ? formatForDiscord(alert)
        : formatForSlack(alert);

      const response = await this._fetchFn(this.url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
        signal: AbortSignal.timeout(5000), // 5s timeout
      });

      if (response.ok) {
        this._stats.sent++;
        this._stats.lastSentAt = new Date().toISOString();
        this._lastEventByType.set(eventType, now);
      } else {
        this._stats.failed++;
        this._stats.lastError = `HTTP ${response.status}: ${response.statusText}`;
        console.warn(`[Webhook] Delivery failed: ${this._stats.lastError}`);
      }
    } catch (err) {
      this._stats.failed++;
      this._stats.lastError = err?.message || String(err);
      console.warn(`[Webhook] Delivery error: ${this._stats.lastError}`);
    }
  }

  // ─── Convenience methods for specific events ──────────────────

  /**
   * Alert: kill-switch activated.
   * @param {{ todayPnl: number, limit: number, overrideCount?: number }} data
   */
  async alertKillSwitch(data) {
    await this.send({
      event: WEBHOOK_EVENTS.KILL_SWITCH,
      title: 'Kill-Switch Activated',
      message: `Daily PnL loss limit reached. Trading halted.`,
      severity: 'critical',
      details: {
        'Today PnL': `$${data.todayPnl?.toFixed(2) ?? '?'}`,
        'Loss Limit': `$${data.limit ?? '?'}`,
        'Override Count': data.overrideCount ?? 0,
      },
    });
  }

  /**
   * Alert: circuit breaker tripped.
   * @param {{ consecutiveLosses: number, cooldownMs: number }} data
   */
  async alertCircuitBreaker(data) {
    await this.send({
      event: WEBHOOK_EVENTS.CIRCUIT_BREAKER,
      title: 'Circuit Breaker Tripped',
      message: `Too many consecutive losses. Trading paused for cooldown.`,
      severity: 'warning',
      details: {
        'Consecutive Losses': data.consecutiveLosses ?? '?',
        'Cooldown': `${((data.cooldownMs ?? 0) / 1000).toFixed(0)}s`,
      },
    });
  }

  /**
   * Alert: order failed after retries.
   * @param {Object} failureEvent - Structured failure event from retryPolicy
   */
  async alertOrderFailed(failureEvent) {
    await this.send({
      event: WEBHOOK_EVENTS.ORDER_FAILED,
      title: 'Order Failed',
      message: `CLOB order failed after ${failureEvent.retryCount ?? 0} retries.`,
      severity: 'critical',
      details: {
        'Order ID': failureEvent.orderId ?? 'N/A',
        'Error': failureEvent.error?.message ?? 'Unknown',
        'Retryable': failureEvent.error?.retryable ? 'Yes' : 'No',
        'HTTP Status': failureEvent.error?.status ?? 'N/A',
      },
    });
  }

  /**
   * Alert: process crash detected.
   * @param {{ error?: string, signal?: string }} data
   */
  async alertCrash(data) {
    await this.send({
      event: WEBHOOK_EVENTS.PROCESS_CRASH,
      title: 'Process Crash Detected',
      message: data.error || 'Process terminated unexpectedly.',
      severity: 'critical',
      details: {
        'Signal': data.signal ?? 'N/A',
        'PID': process.pid,
        'Uptime': `${Math.round(process.uptime())}s`,
      },
    });
  }

  /**
   * Alert: position reconciliation discrepancy.
   * @param {{ discrepancies: Array }} data
   */
  async alertReconciliationDiscrepancy(data) {
    const count = data.discrepancies?.length ?? 0;
    const first = data.discrepancies?.[0];
    await this.send({
      event: WEBHOOK_EVENTS.RECONCILIATION_DISCREPANCY,
      title: 'Position Reconciliation Discrepancy',
      message: `${count} discrepanc${count === 1 ? 'y' : 'ies'} found between local and CLOB positions.`,
      severity: 'warning',
      details: {
        'Count': count,
        'First Issue': first ? `${first.type}: ${first.detail}` : 'N/A',
      },
    });
  }

  // ─── Stats ────────────────────────────────────────────────────

  /**
   * Get delivery stats.
   * @returns {Object}
   */
  getStats() {
    return {
      ...this._stats,
      configured: this.isConfigured(),
      type: this.type,
    };
  }
}

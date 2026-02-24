import test from 'node:test';
import assert from 'node:assert';
import {
  WebhookService,
  WEBHOOK_EVENTS,
  formatForSlack,
  formatForDiscord,
} from '../../src/infrastructure/webhooks/webhookService.js';

// ── Mock fetch ──────────────────────────────────────────────────────

function mockFetch(status = 200) {
  const calls = [];
  const fn = async (url, opts) => {
    calls.push({ url, opts, body: JSON.parse(opts.body) });
    return { ok: status >= 200 && status < 300, status, statusText: 'OK' };
  };
  fn.calls = calls;
  return fn;
}

// ── formatForSlack ──────────────────────────────────────────────────

test('formatForSlack: produces valid Slack payload', () => {
  const alert = {
    title: 'Kill-Switch Activated',
    message: 'Daily loss limit reached',
    severity: 'critical',
    details: { 'Today PnL': '-$50.00', 'Limit': '$50' },
  };

  const payload = formatForSlack(alert);
  assert.ok(payload.blocks);
  assert.ok(Array.isArray(payload.blocks));

  // Header block
  const header = payload.blocks.find(b => b.type === 'header');
  assert.ok(header);
  assert.ok(header.text.text.includes('CRITICAL'));
  assert.ok(header.text.text.includes('Kill-Switch'));

  // Section with message
  const section = payload.blocks.find(b => b.type === 'section' && b.text);
  assert.ok(section);
  assert.ok(section.text.text.includes('Daily loss limit'));

  // Fields section
  const fieldsSection = payload.blocks.find(b => b.type === 'section' && b.fields);
  assert.ok(fieldsSection);
  assert.ok(fieldsSection.fields.length >= 2);
});

test('formatForSlack: handles missing details', () => {
  const payload = formatForSlack({ title: 'Test', message: 'msg', severity: 'info' });
  assert.ok(payload.blocks);
  // Should not have a fields section
  const fieldsSection = payload.blocks.find(b => b.type === 'section' && b.fields);
  assert.strictEqual(fieldsSection, undefined);
});

// ── formatForDiscord ────────────────────────────────────────────────

test('formatForDiscord: produces valid Discord embed', () => {
  const alert = {
    title: 'Circuit Breaker Tripped',
    message: '5 consecutive losses',
    severity: 'warning',
    details: { 'Losses': 5, 'Cooldown': '300s' },
  };

  const payload = formatForDiscord(alert);
  assert.ok(payload.embeds);
  assert.strictEqual(payload.embeds.length, 1);

  const embed = payload.embeds[0];
  assert.ok(embed.title.includes('WARNING'));
  assert.ok(embed.title.includes('Circuit Breaker'));
  assert.ok(embed.description.includes('5 consecutive'));
  assert.ok(embed.fields.length >= 2);
  assert.ok(embed.timestamp);
  assert.strictEqual(embed.color, 0xFFA500); // orange for warning
});

// ── WebhookService: not configured ──────────────────────────────────

test('WebhookService: isConfigured returns false without URL', () => {
  const svc = new WebhookService({ url: null, _forceNew: true });
  assert.strictEqual(svc.isConfigured(), false);
});

test('WebhookService: send does nothing when not configured', async () => {
  const fetch = mockFetch();
  const svc = new WebhookService({ url: null, fetchFn: fetch, _forceNew: true });

  await svc.send({ event: 'test', title: 'Test', severity: 'info' });
  assert.strictEqual(fetch.calls.length, 0);
});

// ── WebhookService: successful delivery ─────────────────────────────

test('WebhookService: sends Slack-formatted payload', async () => {
  const fetch = mockFetch(200);
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    type: 'slack',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 0,
  });

  await svc.alertKillSwitch({ todayPnl: -50, limit: 50, overrideCount: 0 });

  assert.strictEqual(fetch.calls.length, 1);
  assert.strictEqual(fetch.calls[0].url, 'https://hooks.slack.com/test');
  assert.ok(fetch.calls[0].body.blocks); // Slack format
  assert.strictEqual(svc.getStats().sent, 1);
  assert.strictEqual(svc.getStats().failed, 0);
});

test('WebhookService: sends Discord-formatted payload', async () => {
  const fetch = mockFetch(200);
  const svc = new WebhookService({
    url: 'https://discord.com/api/webhooks/test',
    type: 'discord',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 0,
  });

  await svc.alertCircuitBreaker({ consecutiveLosses: 5, cooldownMs: 300000 });

  assert.strictEqual(fetch.calls.length, 1);
  assert.ok(fetch.calls[0].body.embeds); // Discord format
  assert.strictEqual(svc.getStats().sent, 1);
});

// ── WebhookService: failed delivery ─────────────────────────────────

test('WebhookService: handles HTTP error gracefully', async () => {
  const fetch = mockFetch(500);
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 0,
  });

  // Should not throw
  await svc.alertKillSwitch({ todayPnl: -50, limit: 50 });

  assert.strictEqual(svc.getStats().failed, 1);
  assert.ok(svc.getStats().lastError.includes('500'));
});

test('WebhookService: handles network error gracefully', async () => {
  const fetch = async () => { throw new Error('Network unreachable'); };
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 0,
  });

  await svc.alertOrderFailed({
    type: 'ORDER_FAILED',
    orderId: 'test-123',
    error: { message: 'CLOB rejected', retryable: false, status: 400 },
    retryCount: 3,
  });

  assert.strictEqual(svc.getStats().failed, 1);
  assert.ok(svc.getStats().lastError.includes('Network'));
});

// ── WebhookService: deduplication ───────────────────────────────────

test('WebhookService: deduplicates same event type within cooldown', async () => {
  const fetch = mockFetch(200);
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 60_000, // 1 minute
  });

  await svc.alertKillSwitch({ todayPnl: -50, limit: 50 });
  await svc.alertKillSwitch({ todayPnl: -55, limit: 50 }); // should be deduplicated

  assert.strictEqual(fetch.calls.length, 1);
  assert.strictEqual(svc.getStats().sent, 1);
});

test('WebhookService: allows different event types within cooldown', async () => {
  const fetch = mockFetch(200);
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 60_000,
  });

  await svc.alertKillSwitch({ todayPnl: -50, limit: 50 });
  await svc.alertCircuitBreaker({ consecutiveLosses: 5, cooldownMs: 300000 });

  assert.strictEqual(fetch.calls.length, 2);
});

// ── WebhookService: convenience methods ─────────────────────────────

test('WebhookService: alertCrash sends crash event', async () => {
  const fetch = mockFetch(200);
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 0,
  });

  await svc.alertCrash({ error: 'Uncaught exception', signal: 'SIGTERM' });

  assert.strictEqual(fetch.calls.length, 1);
  const body = fetch.calls[0].body;
  const headerText = body.blocks[0].text.text;
  assert.ok(headerText.includes('CRITICAL'));
  assert.ok(headerText.includes('Crash'));
});

test('WebhookService: alertReconciliationDiscrepancy sends warning', async () => {
  const fetch = mockFetch(200);
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    fetchFn: fetch,
    _forceNew: true,
    cooldownMs: 0,
  });

  await svc.alertReconciliationDiscrepancy({
    discrepancies: [
      { type: 'QTY_MISMATCH', detail: 'local=100, clob=90' },
    ],
  });

  assert.strictEqual(fetch.calls.length, 1);
});

// ── WebhookService: getStats ────────────────────────────────────────

test('WebhookService: getStats returns correct structure', () => {
  const svc = new WebhookService({
    url: 'https://hooks.slack.com/test',
    type: 'discord',
    _forceNew: true,
  });

  const stats = svc.getStats();
  assert.strictEqual(stats.configured, true);
  assert.strictEqual(stats.type, 'discord');
  assert.strictEqual(stats.sent, 0);
  assert.strictEqual(stats.failed, 0);
});

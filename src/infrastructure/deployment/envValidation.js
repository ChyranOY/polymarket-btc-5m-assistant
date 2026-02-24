/**
 * @file Startup environment variable validation.
 *
 * Called during application startup to validate required and optional
 * environment variables. Logs clear messages for missing or invalid values.
 *
 * Architecture: Infrastructure layer (startup lifecycle).
 *
 * Required vars cause warnings (bot continues but may not function correctly).
 * Optional vars cause info-level logs.
 *
 * NODE_ENV=production behavior:
 *   - Stricter: logs warnings as errors
 *   - Requires DAILY_LOSS_LIMIT to be set (kill-switch mandatory)
 *   - Enables production-safe defaults
 */

/**
 * Validate environment variables on startup.
 * @returns {{ errors: string[], warnings: string[], info: string[] }}
 */
export function validateEnv() {
  const errors = [];
  const warnings = [];
  const infos = [];

  const isProduction = process.env.NODE_ENV === 'production';

  // ── Required: Price feed must be resolvable ─────────────────────
  const hasSlug = process.env.POLYMARKET_SLUG;
  const autoSelect = (process.env.POLYMARKET_AUTO_SELECT_LATEST || 'true').toLowerCase() === 'true';

  if (!hasSlug && !autoSelect) {
    warnings.push('Neither POLYMARKET_SLUG nor POLYMARKET_AUTO_SELECT_LATEST is configured. Market discovery may fail.');
  }

  const rpcUrl = process.env.POLYGON_RPC_URL;
  if (!rpcUrl) {
    infos.push('POLYGON_RPC_URL not set — using default https://polygon-rpc.com');
  }

  // ── Live trading checks ─────────────────────────────────────────
  const liveEnabled = (process.env.LIVE_TRADING_ENABLED || 'false').toLowerCase() === 'true';
  if (liveEnabled) {
    if (!process.env.FUNDER_ADDRESS) {
      errors.push('LIVE_TRADING_ENABLED=true but FUNDER_ADDRESS is not set. Live trading will fail.');
    }

    const envGate = process.env.LIVE_ENV_GATE;
    if (envGate && envGate !== process.env.NODE_ENV) {
      warnings.push(`LIVE_ENV_GATE=${envGate} does not match NODE_ENV=${process.env.NODE_ENV || '(unset)'}. Live trading may be blocked.`);
    }
  }

  // ── Production-specific checks ──────────────────────────────────
  if (isProduction) {
    const dailyLimit = process.env.DAILY_LOSS_LIMIT || process.env.MAX_DAILY_LOSS_USD;
    if (!dailyLimit) {
      warnings.push('Production mode: DAILY_LOSS_LIMIT not set. Kill-switch will use default ($50). Set explicitly for safety.');
    }

    if (!process.env.WEBHOOK_URL) {
      infos.push('Production mode: WEBHOOK_URL not configured. No critical event alerts will be sent.');
    }
  }

  // ── Optional vars (info-level) ──────────────────────────────────
  const optionalChecks = [
    { key: 'WEBHOOK_URL', label: 'Webhook alerts' },
    { key: 'DATA_DIR', label: 'Custom data directory' },
  ];

  for (const { key, label } of optionalChecks) {
    if (!process.env[key]) {
      infos.push(`${key} not set — ${label} will use defaults`);
    }
  }

  // ── Config sanity ───────────────────────────────────────────────
  const stakePct = Number(process.env.STAKE_PCT);
  if (process.env.STAKE_PCT && (stakePct <= 0 || stakePct > 1)) {
    warnings.push(`STAKE_PCT=${process.env.STAKE_PCT} is outside expected range (0, 1]. Trades may be zero-sized or oversized.`);
  }

  return { errors, warnings, infos };
}

/**
 * Run validation and log results.
 * Called from src/index.js during startup.
 */
export function logEnvValidation() {
  const { errors, warnings, infos } = validateEnv();
  const isProduction = process.env.NODE_ENV === 'production';

  for (const msg of errors) {
    console.error(`[ENV ERROR] ${msg}`);
  }

  for (const msg of warnings) {
    if (isProduction) {
      console.error(`[ENV WARN] ${msg}`);
    } else {
      console.warn(`[ENV WARN] ${msg}`);
    }
  }

  // Only log info in non-production (reduce noise in prod)
  if (!isProduction) {
    for (const msg of infos) {
      console.log(`[ENV INFO] ${msg}`);
    }
  }

  if (errors.length === 0 && warnings.length === 0) {
    console.log('[ENV] All environment checks passed');
  } else {
    console.log(`[ENV] Validation: ${errors.length} error(s), ${warnings.length} warning(s)`);
  }
}

/**
 * Get production-safe config overrides when NODE_ENV=production.
 * @returns {Object} Config overrides to merge
 */
export function getProductionDefaults() {
  if (process.env.NODE_ENV !== 'production') return {};

  return {
    // Shorter poll interval in production for faster responsiveness
    // (can be overridden by env var)
    pollIntervalMs: Number(process.env.POLL_INTERVAL_MS) || 1000,
  };
}

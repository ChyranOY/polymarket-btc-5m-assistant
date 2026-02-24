/**
 * @file Graceful shutdown handler — drain open positions on SIGTERM.
 *
 * On SIGTERM (deployment signal):
 *   1. Stop accepting new trades (tradingEnabled = false)
 *   2. Wait for open position to close (up to 5 min timeout for 5m contracts)
 *   3. Persist critical state
 *   4. Release trading lock
 *   5. Close DB connections
 *   6. Exit cleanly
 *
 * Architecture: Infrastructure layer (process lifecycle).
 */

// ── Default config ─────────────────────────────────────────────────
const DEFAULT_DRAIN_TIMEOUT_MS = 5 * 60_000; // 5 minutes (matches 5m contract window)
const DRAIN_CHECK_INTERVAL_MS = 2000; // Check every 2s

/**
 * Install graceful shutdown handlers for SIGTERM and SIGINT.
 *
 * @param {Object} opts
 * @param {Function} opts.getEngine - Returns the TradingEngine instance
 * @param {Function} [opts.getStateManager] - Returns the StateManager instance
 * @param {Function} [opts.getTradingLock] - Returns the TradingLock instance
 * @param {Function} [opts.getTradeStore] - Returns the TradeStore instance
 * @param {Function} [opts.getWebhookService] - Returns the WebhookService instance
 * @param {Function} [opts.getServer] - Returns the HTTP server instance
 * @param {number} [opts.drainTimeoutMs] - Max drain wait time
 * @param {Function} [opts.onDrainComplete] - Callback after drain (for testing)
 */
export function installGracefulShutdown(opts = {}) {
  const drainTimeoutMs = opts.drainTimeoutMs ?? DEFAULT_DRAIN_TIMEOUT_MS;
  let isShuttingDown = false;

  const handler = async (signal) => {
    if (isShuttingDown) {
      console.log(`[Shutdown] Already shutting down, ignoring ${signal}`);
      return;
    }
    isShuttingDown = true;

    console.log(`\n[Shutdown] Received ${signal}. Starting graceful shutdown...`);

    const engine = opts.getEngine?.();
    const stateManager = opts.getStateManager?.();
    const tradingLock = opts.getTradingLock?.();
    const webhookService = opts.getWebhookService?.();
    const server = opts.getServer?.();

    // Step 1: Stop accepting new trades
    if (engine) {
      engine.tradingEnabled = false;
      console.log('[Shutdown] Trading disabled — no new entries');
    }

    // Step 2: Drain open positions (wait for exit)
    if (engine?.state?.hasOpenPosition) {
      console.log('[Shutdown] Draining open position (waiting for close)...');

      const drainStart = Date.now();
      while (Date.now() - drainStart < drainTimeoutMs) {
        // Check if position closed
        try {
          const positions = await engine.executor?.getOpenPositions?.({});
          if (!positions || positions.length === 0) {
            console.log('[Shutdown] Open position closed — drain complete');
            break;
          }
        } catch {
          // best-effort position check
        }

        await new Promise(resolve => setTimeout(resolve, DRAIN_CHECK_INTERVAL_MS));
      }

      const elapsed = Date.now() - (engine.state.hasOpenPosition ? Date.now() : 0);
      if (engine.state.hasOpenPosition) {
        console.warn(`[Shutdown] Drain timeout after ${(drainTimeoutMs / 1000).toFixed(0)}s — position may still be open`);
      }
    } else {
      console.log('[Shutdown] No open positions — skipping drain');
    }

    // Step 3: Persist critical state
    if (stateManager && engine?.state) {
      try {
        stateManager.shutdown(engine.state);
        console.log('[Shutdown] State persisted');
      } catch (err) {
        console.error('[Shutdown] State persistence failed:', err.message);
      }
    }

    // Step 4: Release trading lock
    if (tradingLock) {
      try {
        tradingLock.releaseLock();
        console.log('[Shutdown] Trading lock released');
      } catch (err) {
        console.error('[Shutdown] Lock release failed:', err.message);
      }
    }

    // Step 5: Close DB connections
    const tradeStore = opts.getTradeStore?.();
    if (tradeStore) {
      try {
        tradeStore.close();
        console.log('[Shutdown] Trade store closed');
      } catch (err) {
        console.error('[Shutdown] Trade store close failed:', err.message);
      }
    }

    // Step 6: Close HTTP server
    if (server) {
      try {
        server.close();
        console.log('[Shutdown] HTTP server closed');
      } catch (err) {
        console.error('[Shutdown] Server close failed:', err.message);
      }
    }

    // Send webhook notification
    if (webhookService?.isConfigured?.()) {
      try {
        await webhookService.send({
          event: 'GRACEFUL_SHUTDOWN',
          title: 'Instance Shutting Down',
          message: `Graceful shutdown initiated by ${signal}`,
          severity: 'info',
          details: {
            'Signal': signal,
            'PID': process.pid,
            'Uptime': `${Math.round(process.uptime())}s`,
          },
        });
      } catch {
        // best-effort
      }
    }

    console.log('[Shutdown] Graceful shutdown complete');

    // Callback for testing
    if (opts.onDrainComplete) {
      opts.onDrainComplete();
      return; // Don't exit in tests
    }

    process.exit(0);
  };

  process.on('SIGTERM', () => handler('SIGTERM'));
  process.on('SIGINT', () => handler('SIGINT'));

  // Handle uncaught exceptions — persist state before crash
  process.on('uncaughtException', async (err) => {
    console.error('[CRASH] Uncaught exception:', err);

    const stateManager = opts.getStateManager?.();
    const engine = opts.getEngine?.();
    const webhookService = opts.getWebhookService?.();

    // Persist state
    if (stateManager && engine?.state) {
      try {
        stateManager.persistState(engine.state, { immediate: true });
      } catch { /* best-effort */ }
    }

    // Send crash alert
    if (webhookService?.isConfigured?.()) {
      try {
        await webhookService.alertCrash({ error: err.message });
      } catch { /* best-effort */ }
    }

    process.exit(1);
  });

  process.on('unhandledRejection', (reason) => {
    console.error('[CRASH] Unhandled rejection:', reason);
    // Don't exit — log and continue
  });

  return { isShuttingDown: () => isShuttingDown };
}

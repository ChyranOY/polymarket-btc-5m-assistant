/* global Chart */

/**
 * Analytics and Optimizer tab rendering.
 * Follows existing vanilla JS patterns from script.js.
 *
 * - Tab navigation between Dashboard, Analytics, Optimizer
 * - Analytics tab: period tables, advanced metrics, drawdown chart
 * - Optimizer tab: parameter form, grid search, sortable results, apply/revert
 */

document.addEventListener('DOMContentLoaded', () => {

  // ── Tab Navigation ──────────────────────────────────────────────

  let activeTab = 'dashboard';
  const tabBtns = document.querySelectorAll('.tab-btn');
  const tabContents = document.querySelectorAll('.tab-content');

  function switchTab(tabName) {
    activeTab = tabName;

    // Update button active state
    tabBtns.forEach(btn => {
      btn.classList.toggle('active', btn.dataset.tab === tabName);
    });

    // Show/hide tab content
    tabContents.forEach(content => {
      const isTarget = content.id === `tab-${tabName}`;
      content.classList.toggle('hidden', !isTarget);
    });

    // Expose activeTab globally for script.js polling guard
    window.__activeTab = tabName;

    // Fetch data for active tab
    if (tabName === 'analytics') {
      fetchAndRenderAnalytics();
      fetchAndRenderSuggestions();
      fetchAndRenderTracking();
    } else if (tabName === 'optimizer') {
      fetchCurrentConfig();
      checkRevertAvailable();
    }
  }

  tabBtns.forEach(btn => {
    btn.addEventListener('click', () => switchTab(btn.dataset.tab));
  });

  // Initialize
  window.__activeTab = 'dashboard';

  // ── Period sub-tab navigation ───────────────────────────────────

  let activePeriod = 'day';
  let cachedAnalytics = null;

  const periodBtns = document.querySelectorAll('.period-tab-btn');
  periodBtns.forEach(btn => {
    btn.addEventListener('click', () => {
      activePeriod = btn.dataset.period;
      periodBtns.forEach(b => b.classList.toggle('active', b === btn));
      if (cachedAnalytics) renderPeriodTable(cachedAnalytics);
    });
  });

  // ── Segmented Performance sub-tab navigation ─────────────────────

  let activeSegment = 'entryPhase';

  const segBtns = document.querySelectorAll('.seg-tab-btn');
  segBtns.forEach(btn => {
    btn.addEventListener('click', () => {
      activeSegment = btn.dataset.seg;
      segBtns.forEach(b => b.classList.toggle('active', b === btn));
      if (cachedAnalytics) renderSegmentedTable(cachedAnalytics);
    });
  });

  // ── Analytics Tab ───────────────────────────────────────────────

  let drawdownChart = null;

  async function fetchAndRenderAnalytics() {
    try {
      const res = await fetch('/api/analytics');
      const json = await res.json();
      if (!json.success) throw new Error(json.error || 'Analytics fetch failed');
      cachedAnalytics = json.data;
      renderPeriodTable(cachedAnalytics);
      renderSegmentedTable(cachedAnalytics);
      renderAdvancedMetrics(cachedAnalytics);
      renderDrawdownChart(cachedAnalytics);
    } catch (err) {
      const container = document.getElementById('period-table-container');
      if (container) container.innerHTML = `<p class="muted-text">Error: ${err.message}</p>`;
    }
  }

  function renderPeriodTable(data) {
    const container = document.getElementById('period-table-container');
    if (!container) return;

    let rows = [];
    if (activePeriod === 'day') {
      rows = data.byDay || [];
    } else if (activePeriod === 'week') {
      rows = data.byWeek || [];
    } else if (activePeriod === 'session') {
      rows = data.bySession || [];
    }

    if (!rows.length) {
      container.innerHTML = '<p class="muted-text">No trade data available for this period.</p>';
      return;
    }

    // Sort by key descending (most recent first) for day/week
    if (activePeriod !== 'session') {
      rows = [...rows].sort((a, b) => (b.key || '').localeCompare(a.key || ''));
    }

    const fmt = (v, d = 2) => v != null ? Number(v).toFixed(d) : '--';
    const fmtPct = (v) => v != null ? (Number(v) * 100).toFixed(1) + '%' : '--';

    let html = `<table class="period-table">
      <thead><tr>
        <th>Period</th><th>Trades</th><th>Wins</th><th>Losses</th>
        <th>Win Rate</th><th>PnL</th><th>Avg PnL</th>
      </tr></thead><tbody>`;

    for (const row of rows) {
      const pnlClass = (row.pnl || 0) >= 0 ? 'positive' : 'negative';
      html += `<tr>
        <td>${row.key || '--'}</td>
        <td>${row.count || 0}</td>
        <td>${row.wins || 0}</td>
        <td>${row.losses || 0}</td>
        <td>${fmtPct(row.winRate)}</td>
        <td class="${pnlClass}">$${fmt(row.pnl)}</td>
        <td>$${fmt(row.avgPnl)}</td>
      </tr>`;
    }

    html += '</tbody></table>';
    container.innerHTML = html;
  }

  function renderSegmentedTable(data) {
    const container = document.getElementById('segmented-table-container');
    if (!container) return;

    let rows = [];
    if (activeSegment === 'entryPhase') {
      rows = data.byEntryPhase || [];
    } else if (activeSegment === 'session') {
      rows = data.bySession || [];
    } else if (activeSegment === 'regime') {
      rows = data.byMarketRegime || [];
    }

    if (!rows.length) {
      container.innerHTML = '<p class="muted-text">No segmented data available.</p>';
      return;
    }

    // Sort by PnL descending (most profitable first)
    rows = [...rows].sort((a, b) => (b.pnl || 0) - (a.pnl || 0));

    const fmt = (v, d = 2) => v != null ? Number(v).toFixed(d) : '--';
    const fmtPct = (v) => v != null ? (Number(v) * 100).toFixed(1) + '%' : '--';

    let html = `<table class="segmented-table">
      <thead><tr>
        <th>Segment</th><th>Trades</th><th>Win Rate</th><th>PF</th>
        <th>PnL</th><th>Avg PnL</th>
      </tr></thead><tbody>`;

    for (const row of rows) {
      if (row.key === 'unknown') continue;
      const pnlClass = (row.pnl || 0) >= 0 ? 'positive' : 'negative';
      const pfClass = row.profitFactor != null ? (row.profitFactor >= 1.0 ? 'pf-good' : 'pf-bad') : '';
      const lowConf = (row.count || 0) < 5 ? 'low-confidence' : '';
      html += `<tr class="${lowConf}">
        <td>${row.key || '--'}</td>
        <td>${row.count || 0}${lowConf ? ' *' : ''}</td>
        <td>${fmtPct(row.winRate)}</td>
        <td class="${pfClass}">${fmt(row.profitFactor)}</td>
        <td class="${pnlClass}">$${fmt(row.pnl)}</td>
        <td>$${fmt(row.avgPnl)}</td>
      </tr>`;
    }

    html += '</tbody></table>';
    if (rows.some(r => (r.count || 0) < 5 && r.key !== 'unknown')) {
      html += '<p class="muted-text" style="margin-top:8px">* Low sample size (< 5 trades)</p>';
    }
    container.innerHTML = html;
  }

  function renderAdvancedMetrics(data) {
    const m = data.advancedMetrics || {};
    const el = (id) => document.getElementById(id);

    const sharpe = el('metric-sharpe');
    const sortino = el('metric-sortino');
    const ddUsd = el('metric-dd-usd');
    const ddPct = el('metric-dd-pct');
    const confidence = el('metric-confidence');

    if (sharpe) sharpe.textContent = m.sharpeRatio != null ? Number(m.sharpeRatio).toFixed(2) : '--';
    if (sortino) sortino.textContent = m.sortinoRatio != null ? Number(m.sortinoRatio).toFixed(2) : '--';
    if (ddUsd) ddUsd.textContent = m.maxDrawdownUsd != null ? '$' + Number(m.maxDrawdownUsd).toFixed(2) : '--';
    if (ddPct) ddPct.textContent = m.maxDrawdownPct != null ? (Number(m.maxDrawdownPct) * 100).toFixed(2) + '%' : '--';

    if (confidence) {
      const isHigh = m.metricsConfidence === 'HIGH';
      confidence.textContent = isHigh ? 'HIGH' : 'LOW';
      confidence.classList.toggle('badge-high', isHigh);
      confidence.classList.toggle('badge-low', !isHigh);
    }
  }

  function renderDrawdownChart(data) {
    const canvas = document.getElementById('chart-drawdown');
    if (!canvas || !window.Chart) return;

    const series = data.advancedMetrics?.drawdownSeries || [];

    if (drawdownChart) {
      drawdownChart.destroy();
      drawdownChart = null;
    }

    const labels = series.map(s => s.tradeIndex);
    const ddData = series.map(s => (s.drawdownPct || 0) * 100);  // as percentage, negative values

    drawdownChart = new Chart(canvas, {
      type: 'line',
      data: {
        labels,
        datasets: [{
          label: 'Drawdown %',
          data: ddData,
          borderColor: '#f85149',
          backgroundColor: 'rgba(248, 81, 73, 0.15)',
          fill: true,
          tension: 0.2,
          pointRadius: 0,
        }]
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: { labels: { color: '#e7eefc', boxWidth: 10 } },
          tooltip: { enabled: true },
        },
        scales: {
          x: {
            title: { display: true, text: 'Trade #', color: '#8b949e' },
            ticks: { color: 'rgba(230,237,243,0.4)' },
            grid: { color: 'rgba(255,255,255,0.06)' },
          },
          y: {
            title: { display: true, text: 'Drawdown %', color: '#8b949e' },
            ticks: { color: 'rgba(230,237,243,0.4)' },
            grid: { color: 'rgba(255,255,255,0.06)' },
          }
        }
      }
    });
  }

  // ── Suggestions ─────────────────────────────────────────────────

  async function fetchAndRenderSuggestions() {
    const container = document.getElementById('suggestions-container');
    const badge = document.getElementById('suggestions-refresh-badge');
    if (!container) return;

    try {
      const res = await fetch('/api/suggestions');
      const json = await res.json();

      if (!json.success) {
        container.innerHTML = `<p class="muted-text">Error loading suggestions.</p>`;
        return;
      }

      const data = json.data;

      // Show refresh badge
      if (badge && data.tradesSinceLastAnalysis != null) {
        if (data.tradesSinceLastAnalysis >= 20) {
          badge.textContent = `${data.tradesSinceLastAnalysis} new trades since last analysis`;
          badge.style.color = 'var(--warn)';
        } else {
          badge.textContent = `${data.tradesSinceLastAnalysis} new trades`;
          badge.style.color = '';
        }
      }

      if (data.insufficient) {
        container.innerHTML = `<p class="muted-text">${data.message} (${data.totalEntryChecks} checks so far)</p>`;
        return;
      }

      const suggestions = data.suggestions || [];
      if (suggestions.length === 0) {
        container.innerHTML = `<p class="muted-text">No improvements found. Current thresholds appear optimal based on available data.</p>`;
        return;
      }

      renderSuggestionCards(suggestions);
    } catch (err) {
      if (container) container.innerHTML = `<p class="muted-text">Error: ${err.message}</p>`;
    }
  }

  function renderSuggestionCards(suggestions) {
    const container = document.getElementById('suggestions-container');
    if (!container) return;

    const fmt = (v, d = 2) => v != null ? Number(v).toFixed(d) : '--';
    const fmtPct = (v) => v != null ? (Number(v) * 100).toFixed(1) + '%' : '--';

    let html = '';
    suggestions.forEach((s, idx) => {
      const confClass = `confidence-${s.confidence}`;

      const wrChange = (s.projected.winRate != null && s.baseline.winRate != null)
        ? s.projected.winRate - s.baseline.winRate : null;
      const pfChange = (s.projected.profitFactor != null && s.baseline.profitFactor != null)
        ? s.projected.profitFactor - s.baseline.profitFactor : null;
      const tcChange = (s.projected.tradeCount != null && s.baseline.tradeCount != null && s.baseline.tradeCount > 0)
        ? ((s.projected.tradeCount - s.baseline.tradeCount) / s.baseline.tradeCount * 100) : null;

      html += `
        <div class="suggestion-card ${confClass}">
          <div class="suggestion-header">
            <span class="confidence-dot ${s.confidence}"></span>
            <span class="suggestion-param-name">${s.label}</span>
          </div>
          <div class="suggestion-values">
            ${fmt(s.currentValue, 4)} <span class="arrow">&rarr;</span> ${fmt(s.suggestedValue, 4)}
          </div>
          <div class="blocker-freq-badge">Blocked ${s.blockerFrequency}% of entries</div>
          <table class="suggestion-metrics-table">
            <thead><tr><th></th><th>Current</th><th>Projected</th><th>Change</th></tr></thead>
            <tbody>
              <tr>
                <td>Win Rate</td>
                <td>${fmtPct(s.baseline.winRate)}</td>
                <td>${fmtPct(s.projected.winRate)}</td>
                <td class="${wrChange != null && wrChange >= 0 ? 'suggestion-change-positive' : 'suggestion-change-negative'}">${wrChange != null ? (wrChange >= 0 ? '+' : '') + fmtPct(wrChange) : '--'}</td>
              </tr>
              <tr>
                <td>PF</td>
                <td>${fmt(s.baseline.profitFactor)}</td>
                <td>${fmt(s.projected.profitFactor)}</td>
                <td class="${pfChange != null && pfChange >= 0 ? 'suggestion-change-positive' : 'suggestion-change-negative'}">${pfChange != null ? (pfChange >= 0 ? '+' : '') + fmt(pfChange) : '--'}</td>
              </tr>
              <tr>
                <td>Trades</td>
                <td>${s.baseline.tradeCount || '--'}</td>
                <td>${s.projected.tradeCount || '--'}</td>
                <td class="${tcChange != null && tcChange >= 0 ? 'suggestion-change-positive' : 'suggestion-change-negative'}">${tcChange != null ? (tcChange >= 0 ? '+' : '') + tcChange.toFixed(0) + '%' : '--'}</td>
              </tr>
            </tbody>
          </table>
          <button class="apply-suggestion-btn" data-idx="${idx}">Apply</button>
        </div>`;
    });

    container.innerHTML = html;

    // Attach apply handlers
    container.querySelectorAll('.apply-suggestion-btn').forEach(btn => {
      btn.addEventListener('click', async () => {
        const idx = Number(btn.dataset.idx);
        const s = suggestions[idx];
        if (!s) return;

        btn.disabled = true;
        btn.textContent = 'Applying...';

        try {
          const res = await fetch('/api/suggestions/apply', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              configKey: s.configKey,
              suggestedValue: s.suggestedValue,
              projected: s.projected,
            }),
          });
          const json = await res.json();
          if (!res.ok || !json.success) throw new Error(json.error || 'Apply failed');

          btn.textContent = 'Applied!';
          btn.style.borderColor = 'var(--good)';

          // Refresh suggestions and tracking
          setTimeout(() => {
            fetchAndRenderSuggestions();
            fetchAndRenderTracking();
          }, 500);
        } catch (err) {
          btn.textContent = 'Failed';
          btn.style.borderColor = 'var(--bad)';
          setTimeout(() => {
            btn.textContent = 'Apply';
            btn.disabled = false;
            btn.style.borderColor = '';
          }, 2000);
        }
      });
    });
  }

  async function fetchAndRenderTracking() {
    const card = document.getElementById('tracking-card');
    const container = document.getElementById('tracking-container');
    if (!card || !container) return;

    try {
      const res = await fetch('/api/suggestions/tracking');
      const json = await res.json();
      if (!json.success) return;

      const tracking = json.data?.tracking || [];
      if (tracking.length === 0) {
        card.style.display = 'none';
        return;
      }

      card.style.display = '';
      const fmt = (v, d = 2) => v != null ? Number(v).toFixed(d) : '--';
      const fmtPct = (v) => v != null ? (Number(v) * 100).toFixed(1) + '%' : '--';

      let html = '';
      for (const t of tracking) {
        const statusClass = t.status === 'underperforming' ? 'status-badge-underperforming' : 'status-badge-ontrack';
        const statusText = t.status === 'underperforming' ? 'Underperforming' : 'On Track';

        html += `
          <div class="tracking-record">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px">
              <span style="font-size:13px;font-weight:600;color:var(--text)">${t.configKey}</span>
              <span class="status-badge ${statusClass}">${statusText}</span>
            </div>
            <div style="font-family:var(--mono);font-size:12px;color:var(--muted);margin-bottom:6px">
              Applied: ${fmt(t.suggestedValue, 4)} | Trades since: ${t.tradesSinceApply}
            </div>
            <table class="suggestion-metrics-table">
              <thead><tr><th></th><th>Projected</th><th>Actual</th></tr></thead>
              <tbody>
                <tr>
                  <td>Win Rate</td>
                  <td>${fmtPct(t.projected.winRate)}</td>
                  <td>${fmtPct(t.actual.winRate)}</td>
                </tr>
                <tr>
                  <td>PF</td>
                  <td>${fmt(t.projected.profitFactor)}</td>
                  <td>${fmt(t.actual.profitFactor)}</td>
                </tr>
              </tbody>
            </table>
          </div>`;
      }

      container.innerHTML = html;
    } catch { /* ignore */ }
  }

  // ── Optimizer Tab ───────────────────────────────────────────────

  const DEFAULT_PARAM_RANGES = {
    minProbMid: { min: 0.50, max: 0.58, step: 0.01 },
    edgeMid: { min: 0.01, max: 0.06, step: 0.01 },
    noTradeRsiMin: { min: 25, max: 40, step: 5 },
    noTradeRsiMax: { min: 40, max: 55, step: 5 },
    maxEntryPolyPrice: { min: 0.004, max: 0.008, step: 0.001 },
  };

  const PARAM_LABELS = {
    minProbMid: 'Min Prob (Mid)',
    edgeMid: 'Edge (Mid)',
    noTradeRsiMin: 'No-Trade RSI Min',
    noTradeRsiMax: 'No-Trade RSI Max',
    maxEntryPolyPrice: 'Max Entry Poly Price',
  };

  let optimizerResults = null;
  let sortColumn = 'profitFactor';
  let sortDirection = 'desc';

  function initOptimizerForm() {
    const formEl = document.getElementById('optimizer-form');
    if (!formEl) return;

    let html = '<div class="param-ranges-grid">';
    for (const [param, range] of Object.entries(DEFAULT_PARAM_RANGES)) {
      const label = PARAM_LABELS[param] || param;
      html += `
        <div class="param-range-group">
          <label class="param-label">${label}</label>
          <div class="param-inputs">
            <label><span>Min</span><input type="number" id="opt-${param}-min" value="${range.min}" step="${range.step}"></label>
            <label><span>Max</span><input type="number" id="opt-${param}-max" value="${range.max}" step="${range.step}"></label>
            <label><span>Step</span><input type="number" id="opt-${param}-step" value="${range.step}" step="${range.step}"></label>
          </div>
        </div>`;
    }
    html += '</div>';
    formEl.innerHTML = html;
  }

  function readFormRanges() {
    const ranges = {};
    for (const param of Object.keys(DEFAULT_PARAM_RANGES)) {
      const min = parseFloat(document.getElementById(`opt-${param}-min`)?.value);
      const max = parseFloat(document.getElementById(`opt-${param}-max`)?.value);
      const step = parseFloat(document.getElementById(`opt-${param}-step`)?.value);
      if (!isNaN(min) && !isNaN(max) && !isNaN(step) && step > 0 && min <= max) {
        ranges[param] = { min, max, step };
      }
    }
    return ranges;
  }

  async function runOptimizer() {
    const statusEl = document.getElementById('optimizer-status');
    const resultsEl = document.getElementById('optimizer-results');
    const runBtn = document.getElementById('run-optimizer');

    if (statusEl) statusEl.textContent = 'Running optimizer...';
    if (runBtn) runBtn.disabled = true;
    if (resultsEl) resultsEl.innerHTML = '<div class="loading-spinner"></div>';

    try {
      const paramRanges = readFormRanges();
      const res = await fetch('/api/optimizer', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ paramRanges }),
      });
      const json = await res.json();

      if (!res.ok || !json.success) {
        throw new Error(json.error || 'Optimizer failed');
      }

      optimizerResults = json.data;

      if (statusEl) {
        statusEl.textContent = `Tested ${optimizerResults.totalCombinations} combinations, skipped ${optimizerResults.skippedCombinations} (< 30 trades)`;
      }

      renderOptimizerResults();
    } catch (err) {
      if (statusEl) statusEl.textContent = `Error: ${err.message}`;
      if (resultsEl) resultsEl.innerHTML = `<p class="muted-text">Optimizer failed: ${err.message}</p>`;
    } finally {
      if (runBtn) runBtn.disabled = false;
    }
  }

  function renderOptimizerResults() {
    const container = document.getElementById('optimizer-results');
    if (!container || !optimizerResults) return;

    const results = optimizerResults.results || [];
    const paramNames = optimizerResults.paramNames || [];

    if (!results.length) {
      container.innerHTML = '<p class="muted-text">No results with enough trades. Need more paper trading history or broaden parameter ranges.</p>';
      return;
    }

    // Sort results
    const sorted = [...results].sort((a, b) => {
      let va = a[sortColumn];
      let vb = b[sortColumn];
      // For params, use first param value as tiebreaker
      if (sortColumn.startsWith('param:')) {
        const pName = sortColumn.slice(6);
        va = a.params?.[pName] ?? 0;
        vb = b.params?.[pName] ?? 0;
      }
      if (va == null) va = -Infinity;
      if (vb == null) vb = -Infinity;
      return sortDirection === 'desc' ? vb - va : va - vb;
    });

    const fmt = (v, d = 2) => v != null ? Number(v).toFixed(d) : '--';
    const fmtPct = (v) => v != null ? (Number(v) * 100).toFixed(1) + '%' : '--';

    // Build columns
    const metricCols = [
      { key: 'tradeCount', label: 'Trades' },
      { key: 'winRate', label: 'Win Rate' },
      { key: 'profitFactor', label: 'PF' },
      { key: 'totalPnl', label: 'Total PnL' },
      { key: 'expectancy', label: 'Expect.' },
    ];

    // Header
    let html = '<table class="optimizer-results-table"><thead><tr>';
    html += '<th>#</th>';
    for (const pName of paramNames) {
      const label = PARAM_LABELS[pName] || pName;
      const sortKey = `param:${pName}`;
      const arrow = sortColumn === sortKey ? (sortDirection === 'desc' ? ' v' : ' ^') : '';
      html += `<th class="sortable" data-sort="${sortKey}">${label}${arrow}</th>`;
    }
    for (const col of metricCols) {
      const arrow = sortColumn === col.key ? (sortDirection === 'desc' ? ' v' : ' ^') : '';
      html += `<th class="sortable" data-sort="${col.key}">${col.label}${arrow}</th>`;
    }
    html += '<th>Apply</th></tr></thead><tbody>';

    // Rows
    sorted.forEach((row, idx) => {
      const isBest = idx === 0;
      const rowClass = isBest ? 'best-combo' : '';
      html += `<tr class="${rowClass}">`;
      html += `<td>${idx + 1}</td>`;

      for (const pName of paramNames) {
        html += `<td>${fmt(row.params?.[pName], 4)}</td>`;
      }

      html += `<td>${row.tradeCount || 0}</td>`;
      html += `<td>${fmtPct(row.winRate)}</td>`;
      html += `<td>${fmt(row.profitFactor)}</td>`;
      html += `<td class="${(row.totalPnl || 0) >= 0 ? 'positive' : 'negative'}">$${fmt(row.totalPnl)}</td>`;
      html += `<td>$${fmt(row.expectancy)}</td>`;
      html += `<td><button class="apply-btn" data-idx="${idx}">Apply</button></td>`;
      html += '</tr>';
    });

    html += '</tbody></table>';
    container.innerHTML = html;

    // Attach sort handlers
    container.querySelectorAll('.sortable').forEach(th => {
      th.addEventListener('click', () => {
        const key = th.dataset.sort;
        if (sortColumn === key) {
          sortDirection = sortDirection === 'desc' ? 'asc' : 'desc';
        } else {
          sortColumn = key;
          sortDirection = 'desc';
        }
        renderOptimizerResults();
      });
    });

    // Attach apply handlers
    container.querySelectorAll('.apply-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        const idx = Number(btn.dataset.idx);
        const sortedResults = [...results].sort((a, b) => {
          let va = a[sortColumn];
          let vb = b[sortColumn];
          if (sortColumn.startsWith('param:')) {
            const pName = sortColumn.slice(6);
            va = a.params?.[pName] ?? 0;
            vb = b.params?.[pName] ?? 0;
          }
          if (va == null) va = -Infinity;
          if (vb == null) vb = -Infinity;
          return sortDirection === 'desc' ? vb - va : va - vb;
        });
        const selectedResult = sortedResults[idx];
        if (selectedResult?.params) {
          applyConfig(selectedResult.params);
        }
      });
    });
  }

  async function applyConfig(params) {
    const statusEl = document.getElementById('optimizer-status');
    try {
      const res = await fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ params }),
      });
      const json = await res.json();
      if (!res.ok || !json.success) throw new Error(json.error || 'Apply failed');

      const data = json.data;
      let msg = 'Config applied successfully!';
      if (data.warning) msg += ' WARNING: ' + data.warning;

      if (statusEl) {
        statusEl.textContent = msg;
        statusEl.classList.add('apply-success');
        setTimeout(() => statusEl.classList.remove('apply-success'), 3000);
      }

      // Show revert button
      showRevertButton(true);
      fetchCurrentConfig();
    } catch (err) {
      if (statusEl) statusEl.textContent = `Apply failed: ${err.message}`;
    }
  }

  async function revertConfig() {
    const statusEl = document.getElementById('optimizer-status');
    try {
      const res = await fetch('/api/config/revert', { method: 'POST' });
      const json = await res.json();
      if (!res.ok || !json.success) throw new Error(json.error || 'Revert failed');

      if (statusEl) statusEl.textContent = 'Config reverted to previous values.';
      showRevertButton(false);
      fetchCurrentConfig();
    } catch (err) {
      if (statusEl) statusEl.textContent = `Revert failed: ${err.message}`;
    }
  }

  function showRevertButton(show) {
    const btn = document.getElementById('revert-config');
    if (btn) btn.style.display = show ? 'inline-block' : 'none';
  }

  async function checkRevertAvailable() {
    try {
      const res = await fetch('/api/config/current');
      const json = await res.json();
      if (json.success && json.data?.revertAvailable) {
        showRevertButton(true);
      }
    } catch { /* ignore */ }
  }

  async function fetchCurrentConfig() {
    const container = document.getElementById('current-config-display');
    if (!container) return;

    try {
      const res = await fetch('/api/config/current');
      const json = await res.json();
      if (!json.success) throw new Error(json.error || 'Config fetch failed');

      const config = json.data?.currentConfig || {};
      const entries = Object.entries(config);

      if (!entries.length) {
        container.innerHTML = '<p class="muted-text">Engine not initialized. Start the server to see current config.</p>';
        return;
      }

      let html = '<table class="config-table"><tbody>';
      for (const [key, value] of entries) {
        const label = PARAM_LABELS[key] || key;
        html += `<tr><td class="k">${label}</td><td class="v">${value != null ? value : '--'}</td></tr>`;
      }
      html += '</tbody></table>';
      container.innerHTML = html;
    } catch (err) {
      container.innerHTML = `<p class="muted-text">${err.message}</p>`;
    }
  }

  // ── Event Bindings ──────────────────────────────────────────────

  const runBtn = document.getElementById('run-optimizer');
  if (runBtn) runBtn.addEventListener('click', runOptimizer);

  const revertBtn = document.getElementById('revert-config');
  if (revertBtn) revertBtn.addEventListener('click', revertConfig);

  // Initialize optimizer form with default ranges
  initOptimizerForm();
});

const POLL_MS = 1500;

const $ = (id) => document.getElementById(id);

function getToken() {
  return localStorage.getItem('controlToken') || '';
}
function setToken(v) {
  if (v) localStorage.setItem('controlToken', v);
  else localStorage.removeItem('controlToken');
}

async function api(path, { method = 'GET', body } = {}) {
  const headers = { 'Content-Type': 'application/json' };
  const token = getToken();
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(path, { method, headers, body: body ? JSON.stringify(body) : undefined });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${res.status} ${text}`);
  }
  const ct = res.headers.get('content-type') || '';
  if (ct.includes('application/json')) return res.json();
  return res.text();
}

function fmtUsd(n) {
  if (n == null || n === '') return '—';
  const num = typeof n === 'string' ? parseFloat(n) : n;
  if (!Number.isFinite(num)) return '—';
  const sign = num < 0 ? '−' : '';
  return `${sign}$${Math.abs(num).toFixed(2)}`;
}

function fmtTimeAgo(ms) {
  if (ms == null) return '—';
  if (ms < 1500) return 'just now';
  if (ms < 60_000) return `${Math.round(ms / 1000)}s ago`;
  return `${Math.round(ms / 60_000)}m ago`;
}

function fmtTimeLeft(sec) {
  if (sec == null) return '—';
  if (sec < 0) return '[ SETTLED ]';
  const m = Math.floor(sec / 60);
  const s = sec - m * 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function fmtTimestamp(iso) {
  if (!iso) return '—';
  const d = new Date(iso);
  return isNaN(d) ? '—' : d.toLocaleTimeString();
}

function setPill(el, active, text) {
  el.textContent = text;
  el.classList.toggle('active', active);
  el.classList.toggle('stopped', !active);
}

function setSignedValue(el, value, kind = 'hero') {
  const num = value == null ? null : parseFloat(value);
  const formatted = fmtUsd(value);
  el.textContent = formatted;
  const posClass = kind === 'hero' ? 'pos' : 'pos';
  const negClass = kind === 'hero' ? 'neg' : 'neg';
  el.classList.remove(posClass, negClass);
  if (num == null || !Number.isFinite(num)) return;
  if (num > 0) el.classList.add(posClass);
  else if (num < 0) el.classList.add(negClass);
}

// Track whether a position was open last poll so we can detect close→refresh trades.
let hadPositionLastPoll = false;

async function renderStatus() {
  const s = await api('/status');
  $('market-slug').textContent = s.market?.slug || '—';
  $('time-left').textContent = fmtTimeLeft(s.market?.time_left_sec);
  $('balance').textContent = fmtUsd(s.balance?.available_usd);
  setSignedValue($('daily-pnl'), s.daily_pnl, 'hero');
  setSignedValue($('realized-pnl'), s.realized_pnl, 'inline');
  $('last-tick').textContent = fmtTimeAgo(s.last_tick_ms_ago);
  $('last-skip').textContent = s.last_skip ? `[ ${s.last_skip.toUpperCase()} ]` : (s.trading_enabled ? '[ ELIGIBLE ]' : '—');
  const cbCooling = !!s.circuit_breaker?.cooldown_until;
  const losses = s.circuit_breaker?.consecutive_losses ?? 0;
  $('circuit').textContent = cbCooling ? `COOLING · ${losses}L` : `OK · ${losses}L`;
  $('circuit').className = cbCooling ? 'value neg' : 'value';
  setPill($('trading-pill'), !!s.trading_enabled, s.trading_enabled ? 'Active' : 'Stopped');

  // Trade stats
  $('total-trades').textContent = s.total_trades != null ? `${s.total_trades} · ${s.wins ?? 0}W` : '—';
  $('win-rate').textContent = s.win_rate != null ? `${(s.win_rate * 100).toFixed(1)}%` : '—';

  if (!window.__modeDirty) {
    $('mode-select').value = s.mode || 'paper';
  }

  renderGates(s.gates);
  renderPosition(s.position, s.unrealized_pnl);

  const hasPositionNow = !!s.position;
  if (hadPositionLastPoll && !hasPositionNow) {
    renderTrades();
  }
  hadPositionLastPoll = hasPositionNow;
}

function renderGates(report) {
  const list = $('gates-list');
  const summary = $('gates-summary');
  if (!report || !Array.isArray(report.gates) || report.gates.length === 0) {
    list.innerHTML = `<li class="gates-empty">[ WAITING FOR SERVER ]</li>`;
    summary.textContent = '—';
    summary.className = 'meta';
    return;
  }
  const failed = report.gates.filter(g => !g.pass).map(g => g.name);
  if (report.all_pass) {
    summary.textContent = '[ ALL PASS ]';
    summary.className = 'meta pos';
  } else {
    summary.textContent = `[ ${failed.length} BLOCKED ]`;
    summary.className = 'meta neg';
  }
  list.innerHTML = report.gates.map(g => `
    <li class="gate ${g.pass ? 'pass' : 'fail'}">
      <span class="gate-icon">${g.pass ? '[+]' : '[×]'}</span>
      <span class="gate-name">${g.name.replace(/_/g, ' ')}</span>
      <span class="gate-detail">${g.detail ? g.detail : ''}</span>
    </li>
  `).join('');
}

function renderPosition(p, unrealized) {
  const tbody = $('position-body');
  if (!p) {
    tbody.innerHTML = `<tr><td colspan="7" class="muted center">[ NONE ]</td></tr>`;
    return;
  }
  const upnl = unrealized ?? p.unrealized_pnl;
  const pnlCls = upnl == null ? '' : (parseFloat(upnl) >= 0 ? 'pos' : 'neg');
  tbody.innerHTML = `
    <tr>
      <td>${fmtTimestamp(p.entry_time)}</td>
      <td>${p.side}</td>
      <td>${Number(p.entry_price).toFixed(4)}</td>
      <td>${Number(p.shares).toFixed(2)}</td>
      <td>${fmtUsd(p.contract_size)}</td>
      <td class="pos-pnl ${pnlCls}">${fmtUsd(upnl)}</td>
      <td class="muted">${p.market_slug}</td>
    </tr>
  `;
}

async function renderTrades() {
  try {
    const rows = await api('/trades?limit=20');
    const tbody = $('trades-body');
    if (!Array.isArray(rows) || rows.length === 0) {
      tbody.innerHTML = `<tr><td colspan="8" class="muted center">[ NO TRADES ]</td></tr>`;
      return;
    }
    tbody.innerHTML = rows.map(r => {
      const pnl = r.pnl ?? null;
      const pnlCls = pnl == null ? '' : (parseFloat(pnl) >= 0 ? 'pos' : 'neg');
      const reason = r.exitReason || (r.status === 'OPEN' ? 'OPEN' : '—');
      // Strategy lives inside entryGateSnapshot JSON ("strategy" field).
      // Older rows (pre-tag) will show CHEAP by default.
      let strategy = 'CHEAP';
      const snap = r.entryGateSnapshot || r.entry_gate_snapshot;
      if (snap) {
        try {
          const parsed = typeof snap === 'string' ? JSON.parse(snap) : snap;
          if (parsed && parsed.strategy) {
            strategy = String(parsed.strategy).toUpperCase();
          }
        } catch {}
      }
      return `
        <tr>
          <td>${fmtTimestamp(r.entryTime || r.entry_time)}</td>
          <td>${(r.mode || '—').toUpperCase()}</td>
          <td class="strategy-${strategy.toLowerCase()}">${strategy}</td>
          <td>${r.side || '—'}</td>
          <td>${r.entryPrice != null ? Number(r.entryPrice).toFixed(4) : '—'}</td>
          <td>${r.exitPrice != null ? Number(r.exitPrice).toFixed(4) : '—'}</td>
          <td>${r.shares != null ? Number(r.shares).toFixed(2) : '—'}</td>
          <td class="pos-pnl ${pnlCls}">${fmtUsd(pnl)}</td>
          <td class="muted">${reason}</td>
        </tr>
      `;
    }).join('');
  } catch (e) {
    console.warn('trades fetch failed', e);
  }
}

// Published by the GitHub Actions cron to an orphan `data` branch so daily
// updates don't trigger a DO redeploy. raw.githubusercontent.com serves with
// permissive CORS, so we can fetch directly from the dashboard.
const KRONOS_URL = 'https://raw.githubusercontent.com/ElijahPrince73/polymarket-btc-5m-assistant/data/kronos_daily.json';

async function renderKronos() {
  let data;
  try {
    const res = await fetch(KRONOS_URL, { cache: 'no-store' });
    if (!res.ok) throw new Error(res.status);
    data = await res.json();
  } catch {
    // Not generated yet — keep the empty-state placeholder visible.
    return;
  }

  const meta = $('kronos-meta');
  const genAt = data.generated_at ? new Date(data.generated_at) : null;
  meta.textContent = genAt
    ? `[ ${genAt.toISOString().slice(0, 10)} · ${data.model || ''} ]`.toUpperCase()
    : '—';

  if (!data.metrics || !Array.isArray(data.trades) || data.trades.length === 0) {
    $('kronos-empty').style.display = '';
    $('kronos-body').style.display = 'none';
    $('kronos-empty').textContent = data.note
      ? `[ ${String(data.note).toUpperCase()} ]`
      : '[ NO TRADES IN WINDOW ]';
    return;
  }

  $('kronos-empty').style.display = 'none';
  $('kronos-body').style.display = '';

  const m = data.metrics;
  $('kronos-n').textContent = m.n;
  $('kronos-agree').textContent = `${(m.agreement_rate * 100).toFixed(0)}%`;
  $('kronos-ll-market').textContent = m.market_log_loss.toFixed(4);
  $('kronos-ll-kronos').textContent = m.kronos_log_loss.toFixed(4);
  // Highlight the lower (better) log-loss.
  $('kronos-ll-market').className = 'value';
  $('kronos-ll-kronos').className = 'value';
  if (m.kronos_log_loss < m.market_log_loss) $('kronos-ll-kronos').classList.add('pos');
  else $('kronos-ll-market').classList.add('pos');

  const cf = m.counterfactual_total_per_dollar;
  $('kronos-cf').textContent = `${cf >= 0 ? '+' : ''}${cf.toFixed(2)}`;
  $('kronos-cf').className = `value ${cf >= 0 ? 'pos' : 'neg'}`;
  setSignedValue($('kronos-actual'), m.actual_pnl_usd, 'inline');

  const rows = data.trades.map((t) => {
    const agreeCls = t.agreement ? 'pos' : 'neg';
    const outcome = t.outcome_up ? 'UP' : 'DOWN';
    const pnlCls = t.pnl == null ? '' : (parseFloat(t.pnl) >= 0 ? 'pos' : 'neg');
    return `
      <tr>
        <td>${fmtTimestamp(t.entry_time)}</td>
        <td>${t.side}</td>
        <td>${Number(t.market_p_up).toFixed(3)}</td>
        <td>${Number(t.kronos_p_up).toFixed(3)}</td>
        <td>${outcome}</td>
        <td class="${agreeCls}">${t.agreement ? '[+]' : '[×]'}</td>
        <td class="pos-pnl ${pnlCls}">${fmtUsd(t.pnl)}</td>
      </tr>
    `;
  }).join('');
  $('kronos-trades-body').innerHTML = rows;
}

async function renderVersion() {
  try {
    const h = await api('/health');
    $('version').textContent = `v${h.version}`;
  } catch {}
}

async function postToggle(path) {
  try {
    await api(path, { method: 'POST' });
    await renderStatus();
  } catch (e) {
    alert(`toggle failed: ${e.message}`);
  }
}

async function postMode(mode) {
  window.__modeDirty = true;
  try {
    await api('/mode', { method: 'POST', body: { mode } });
    await renderStatus();
  } catch (e) {
    alert(`mode change failed: ${e.message}`);
  } finally {
    setTimeout(() => { window.__modeDirty = false; }, 2000);
  }
}

function wire() {
  $('btn-start').addEventListener('click', () => postToggle('/trading/start'));
  $('btn-stop').addEventListener('click', () => postToggle('/trading/stop'));
  $('mode-select').addEventListener('change', (e) => postMode(e.target.value));
  const input = $('control-token');
  input.value = getToken();
  input.addEventListener('change', () => setToken(input.value.trim()));
  const refresh = document.getElementById('btn-refresh-trades');
  if (refresh) refresh.addEventListener('click', renderTrades);
  wireTabs();
}

function wireTabs() {
  const buttons = document.querySelectorAll('.tab-button');
  const panels = document.querySelectorAll('.tab-panel');
  const refreshBtn = document.getElementById('btn-refresh-trades');
  const kronosMeta = document.getElementById('kronos-meta');
  buttons.forEach((btn) => {
    btn.addEventListener('click', () => {
      const tab = btn.dataset.tab;
      buttons.forEach((b) => {
        const active = b.dataset.tab === tab;
        b.classList.toggle('active', active);
        b.setAttribute('aria-selected', active ? 'true' : 'false');
      });
      panels.forEach((p) => {
        p.hidden = p.dataset.tabPanel !== tab;
      });
      if (refreshBtn) refreshBtn.style.display = tab === 'trades' ? '' : 'none';
      if (kronosMeta) kronosMeta.style.display = tab === 'kronos' ? '' : 'none';
    });
  });
}

async function statusTick() {
  try {
    await renderStatus();
  } catch (e) {
    console.warn('status poll error', e);
  } finally {
    setTimeout(statusTick, POLL_MS);
  }
}

wire();
renderVersion();
renderKronos();
renderTrades();
statusTick();

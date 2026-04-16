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
  const sign = num < 0 ? '-' : '';
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
  if (sec < 0) return 'settled';
  const m = Math.floor(sec / 60);
  const s = sec - m * 60;
  return `${m}:${String(s).padStart(2, '0')} left`;
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

// Track whether a position was open last poll so we can detect close→refresh trades.
let hadPositionLastPoll = false;

async function renderStatus() {
  const s = await api('/status');
  $('market-slug').textContent = s.market?.slug || '—';
  $('time-left').textContent = fmtTimeLeft(s.market?.time_left_sec);
  $('balance').textContent = fmtUsd(s.balance?.available_usd);
  $('daily-pnl').textContent = fmtUsd(s.daily_pnl);
  $('last-tick').textContent = fmtTimeAgo(s.last_tick_ms_ago);
  $('last-skip').textContent = s.last_skip || (s.trading_enabled ? 'ELIGIBLE' : '—');
  $('circuit').textContent = s.circuit_breaker?.cooldown_until
    ? `COOLING (losses: ${s.circuit_breaker.consecutive_losses})`
    : `OK (losses: ${s.circuit_breaker?.consecutive_losses ?? 0})`;
  setPill($('trading-pill'), !!s.trading_enabled, s.trading_enabled ? 'ACTIVE' : 'STOPPED');

  // Trade stats
  $('total-trades').textContent = s.total_trades != null ? `${s.total_trades} (${s.wins ?? 0}W)` : '—';
  $('win-rate').textContent = s.win_rate != null ? `${(s.win_rate * 100).toFixed(1)}%` : '—';

  if (!window.__modeDirty) {
    $('mode-select').value = s.mode || 'paper';
  }

  renderPosition(s.position);

  // Trade list refresh logic: only when a position just closed (had one → now none).
  // Avoids hammering /trades (and Supabase) on every status poll.
  const hasPositionNow = !!s.position;
  if (hadPositionLastPoll && !hasPositionNow) {
    // Fire-and-forget so the status tick stays snappy.
    renderTrades();
  }
  hadPositionLastPoll = hasPositionNow;
}

function renderPosition(p) {
  const tbody = $('position-body');
  if (!p) {
    tbody.innerHTML = `<tr><td colspan="7" class="muted center">no open position</td></tr>`;
    return;
  }
  const upnl = p.unrealized_pnl;
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
      tbody.innerHTML = `<tr><td colspan="8" class="muted center">no trades yet</td></tr>`;
      return;
    }
    tbody.innerHTML = rows.map(r => {
      const pnl = r.pnl ?? null;
      const pnlCls = pnl == null ? '' : (parseFloat(pnl) >= 0 ? 'pos' : 'neg');
      return `
        <tr>
          <td>${fmtTimestamp(r.entryTime || r.entry_time)}</td>
          <td>${r.mode || '—'}</td>
          <td>${r.side || '—'}</td>
          <td>${r.entryPrice ? Number(r.entryPrice).toFixed(4) : '—'}</td>
          <td>${r.exitPrice ? Number(r.exitPrice).toFixed(4) : '—'}</td>
          <td>${r.shares ? Number(r.shares).toFixed(2) : '—'}</td>
          <td class="pos-pnl ${pnlCls}">${fmtUsd(pnl)}</td>
          <td class="muted">${r.exitReason || (r.status === 'OPEN' ? 'open' : '—')}</td>
        </tr>
      `;
    }).join('');
  } catch (e) {
    console.warn('trades fetch failed', e);
  }
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
  // Manual refresh button for the trades table — useful if Supabase got out of sync.
  const refresh = document.getElementById('btn-refresh-trades');
  if (refresh) refresh.addEventListener('click', renderTrades);
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
// Trades table: one initial fetch + refresh on position-closed transitions.
// Manual refresh button in the UI for everything else.
renderTrades();
statusTick();

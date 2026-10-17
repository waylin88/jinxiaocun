/* ===========================================================
   进销存 · 前端应用（无框架，移动优先 + 离线可用）
   =========================================================== */

'use strict';

/* ---------- 常量 ---------- */
const TOKEN_KEY = 'jxc_token';
const USER_KEY = 'jxc_user';
const DRAFT_IN_KEY = 'jxc_draft_inbound';

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));

const esc = (s) =>
  String(s ?? '').replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])
  );

const nowLocal = () => {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
};

const shortTime = (s) => (s ? String(s).slice(5, 16).replace('T', ' ') : '');
const num = (n) => Number(n || 0).toLocaleString('zh-CN');

function uuid() {
  if (crypto.randomUUID) return crypto.randomUUID();
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === 'x' ? r : (r & 0x3) | 0x8).toString(16);
  });
}

/* ---------- Toast ---------- */
let toastTimer;
function toast(msg, kind = '') {
  const el = $('#toast');
  el.textContent = msg;
  el.className = 'toast' + (kind ? ' ' + kind : '');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add('hidden'), 2600);
}

/* =========================================================
   IndexedDB 离线队列
   —— 所有写操作先落本地队列，再尝试发服务端；
      断网时留在队列，联网后自动重放，靠 op_id 幂等去重。
   ========================================================= */
const DB_NAME = 'jxc';
const DB_VER = 1;
let _db = null;

function openDB() {
  if (_db) return Promise.resolve(_db);
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VER);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains('queue')) {
        const s = db.createObjectStore('queue', { keyPath: 'op_id' });
        s.createIndex('seq', 'seq');
      }
      if (!db.objectStoreNames.contains('kv')) db.createObjectStore('kv');
    };
    req.onsuccess = () => { _db = req.result; resolve(_db); };
    req.onerror = () => reject(req.error);
  });
}

async function idbPut(store, val, key) {
  const db = await openDB();
  return new Promise((res, rej) => {
    const tx = db.transaction(store, 'readwrite');
    const r = key !== undefined ? tx.objectStore(store).put(val, key) : tx.objectStore(store).put(val);
    r.onsuccess = () => res(r.result);
    r.onerror = () => rej(r.error);
  });
}

async function idbGetAll(store) {
  const db = await openDB();
  return new Promise((res, rej) => {
    const r = db.transaction(store, 'readonly').objectStore(store).getAll();
    r.onsuccess = () => res(r.result || []);
    r.onerror = () => rej(r.error);
  });
}

async function idbDel(store, key) {
  const db = await openDB();
  return new Promise((res, rej) => {
    const r = db.transaction(store, 'readwrite').objectStore(store).delete(key);
    r.onsuccess = () => res();
    r.onerror = () => rej(r.error);
  });
}

async function idbGet(store, key) {
  const db = await openDB();
  return new Promise((res, rej) => {
    const r = db.transaction(store, 'readonly').objectStore(store).get(key);
    r.onsuccess = () => res(r.result);
    r.onerror = () => rej(r.error);
  });
}

const Queue = {
  async all() {
    const list = await idbGetAll('queue');
    return list.sort((a, b) => a.seq - b.seq);
  },
  async push(opType, payload) {
    const op = { op_id: uuid(), op_type: opType, payload, seq: Date.now() };
    await idbPut('queue', op);
    await refreshBadge();
    return op;
  },
  async remove(opId) { await idbDel('queue', opId); },
  async clear() {
    const list = await this.all();
    for (const o of list) await idbDel('queue', o.op_id);
    await refreshBadge();
  },
  async size() { return (await this.all()).length; },
};

const KV = {
  async set(k, v) { return idbPut('kv', v, k); },
  async get(k) { return idbGet('kv', k); },
  async del(k) { return idbDel('kv', k); },
};

/* =========================================================
   API 封装
   ========================================================= */
async function api(path, opts = {}) {
  const token = localStorage.getItem(TOKEN_KEY);
  const headers = { 'Content-Type': 'application/json' };
  if (token) headers.Authorization = 'Bearer ' + token;
  if (opts.headers) Object.assign(headers, opts.headers);

  const res = await fetch(path, {
    method: opts.method || 'GET',
    headers,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });

  let json = null;
  try { json = await res.json(); } catch (_) { /* 非 JSON 响应 */ }

  if (res.status === 401) {
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USER_KEY);
    if (currentView !== 'login') switchPage('login');
    throw new Error('登录已失效');
  }
  if (!res.ok) throw new Error(json?.message || ('请求失败 ' + res.status));
  return json ? json.data : null;
}

/**
 * 写操作统一入口：能连就走服务端，不能连就进离线队列。
 * 返回 { queued: bool, data }
 */
async function write(opType, payload) {
  if (!navigator.onLine) {
    await Queue.push(opType, payload);
    return { queued: true };
  }
  try {
    const data = await api(pathFor(opType), { method: 'POST', body: payload });
    return { queued: false, data };
  } catch (e) {
    if (e.message === '登录已失效') throw e;
    // 网络层错误 → 进队列；业务错误（库存不足等）直接抛出
    if (!navigator.onLine || /Failed to fetch|NetworkError|Load failed/i.test(e.message)) {
      await Queue.push(opType, payload);
      return { queued: true };
    }
    throw e;
  }
}

function pathFor(opType) {
  return opType === 'inbound_create' ? '/api/inbound' : '/api/outbound';
}

/* =========================================================
   同步：把队列推给服务端
   ========================================================= */
let syncing = false;

async function syncNow(silent = false) {
  if (syncing) return;
  const ops = await Queue.all();
  if (!ops.length) { await refreshBadge(); return; }
  if (!navigator.onLine) { if (!silent) toast('当前离线，队列已保存'); return; }

  syncing = true;
  try {
    const resp = await api('/api/sync/push', { method: 'POST', body: { ops } });
    let ok = 0;
    const failed = [];
    for (const r of resp.results) {
      if (r.status === 'ok') { await Queue.remove(r.op_id); ok++; }
      else { failed.push(r); }
    }
    await refreshBadge();
    if (ok && !silent) toast(`已同步 ${ok} 条`, 'ok');
    if (failed.length) {
      // 失败的操作保留在队列里，用户可在"待同步队列"里看到原因并手动处理
      await KV.set('last_sync_errors', failed.map((f) => ({ op_id: f.op_id, message: f.message })));
      if (!silent) toast(`${failed.length} 条同步失败，详见队列`, 'err');
    }
  } catch (e) {
    if (!silent) toast('同步失败：' + e.message, 'err');
  } finally {
    syncing = false;
  }
}

async function refreshBadge() {
  const n = await Queue.size();
  const el = $('#pending-badge');
  el.textContent = `${n} 待同步`;
  el.classList.toggle('hidden', n === 0);
}

/* ---------- 网络状态 ---------- */
function updateNet() {
  const bar = $('#netbar');
  if (navigator.onLine) {
    bar.classList.add('hidden');
    syncNow(true);
  } else {
    bar.textContent = '离线模式 · 操作已保存，联网后自动同步';
    bar.classList.remove('hidden');
  }
}

/* =========================================================
   应用状态与导航
   ========================================================= */
let currentView = 'login';
let currentTab = 'dash';
const Draft = { boxes: [], waybill: '', supplier: '', remark: '' };

function switchPage(name) {
  currentView = name;
  $('#page-login').classList.toggle('active', name === 'login');
  $('#page-login').classList.toggle('hidden', name !== 'login');
  $('#page-app').classList.toggle('active', name === 'app');
  $('#page-app').classList.toggle('hidden', name !== 'app');
}

function setTitle(t) { $('#page-title').textContent = t; }

const Views = {
  dash: viewDash,
  in: viewInbound,
  out: viewOutbound,
  stock: viewStock,
  products: viewProducts,
  inbound: viewInboundList,
  outbound: viewOutboundList,
  pending: viewPending,
  password: viewPassword,
};

async function go(name) {
  currentTab = name;
  $$('.tab').forEach((t) => t.classList.toggle('active', t.dataset.tab === name));
  closeSheet();
  const titles = {
    dash: '工作台', in: '入库点数', out: '出库开单', stock: '库存查询',
    products: '型号管理', inbound: '入库单', outbound: '出库单',
    pending: '待同步队列', password: '修改密码',
  };
  setTitle(titles[name] || '');
  const v = $('#view');
  v.innerHTML = '<div class="empty"><div class="empty-text">加载中…</div></div>';
  try {
    await Views[name]();
  } catch (e) {
    v.innerHTML = `<div class="empty"><div class="empty-text">加载失败：${esc(e.message)}</div></div>`;
  }
}

/* ---------- 底部弹层 ---------- */
function openSheet() {
  const u = JSON.parse(localStorage.getItem(USER_KEY) || '{}');
  $('#sheet-name').textContent = u.name || '未登录';
  $('#sheet-role').textContent = u.role === 'admin' ? '管理员' : '操作员';
  $('#sheet').classList.remove('hidden');
}
function closeSheet() { $('#sheet').classList.add('hidden'); }

/* =========================================================
   视图：工作台
   ========================================================= */
async function viewDash() {
  const v = $('#view');
  let d;
  try { d = await api('/api/dashboard'); }
  catch (e) { v.innerHTML = `<div class="empty"><div class="empty-text">${esc(e.message)}</div></div>`; return; }

  const pending = await Queue.size();

  v.innerHTML = `
    <div class="stat-grid">
      <div class="stat is-in">
        <div class="stat-label">今日入库</div>
        <div class="stat-value">${num(d.today_in_qty)}<span class="stat-unit">件</span></div>
      </div>
      <div class="stat is-out">
        <div class="stat-label">今日出库</div>
        <div class="stat-value">${num(d.today_out_qty)}<span class="stat-unit">件</span></div>
      </div>
      <div class="stat">
        <div class="stat-label">在库总量</div>
        <div class="stat-value">${num(d.stock_total)}<span class="stat-unit">件</span></div>
      </div>
      <div class="stat ${d.low_stock_count > 0 ? 'is-alert' : ''}">
        <div class="stat-label">低库存预警</div>
        <div class="stat-value">${num(d.low_stock_count)}<span class="stat-unit">种</span></div>
      </div>
    </div>

    <div class="card" style="margin-top:12px">
      <div class="card-head"><span class="card-title">快捷操作</span></div>
      <div style="display:grid;grid-template-columns:1fr 1fr;gap:9px">
        <button class="btn btn-primary" data-act="go-in">入库点数</button>
        <button class="btn" data-act="go-out">出库开单</button>
        <button class="btn btn-ghost" data-act="go-stock">库存查询</button>
        <button class="btn btn-ghost" data-act="go-products">型号管理</button>
      </div>
    </div>

    <div class="card">
      <div class="card-head"><span class="card-title">今日单据</span>
        <span class="card-sub">入 ${d.today_in_orders} · 出 ${d.today_out_orders}</span></div>
      <div class="list">
        <button class="row" style="text-align:left;width:100%" data-act="go-inbound">
          <div class="row-main"><div><div class="row-title">入库单列表</div>
          <div class="row-meta">按运单号查看每箱的型号与数量${d.pending_draft ? ` · ${d.pending_draft} 张草稿` : ''}</div></div></div>
        </button>
        <button class="row" style="text-align:left;width:100%" data-act="go-outbound">
          <div class="row-main"><div><div class="row-title">出库单列表</div>
          <div class="row-meta">按客户查看出货明细</div></div></div>
        </button>
        <button class="row" style="text-align:left;width:100%" data-act="go-pending">
          <div class="row-main"><div><div class="row-title">待同步队列</div>
          <div class="row-meta">${pending ? `${pending} 条离线操作等待上传` : '暂无待同步数据'}</div></div></div>
        </button>
      </div>
    </div>`;

  v.onclick = (e) => {
    const a = e.target.closest('[data-act]');
    if (!a) return;
    const m = { 'go-in': 'in', 'go-out': 'out', 'go-stock': 'stock', 'go-products': 'products', 'go-inbound': 'inbound', 'go-outbound': 'outbound', 'go-pending': 'pending' };
    go(m[a.dataset.act]);
  };
}

/* =========================================================
   视图：入库点数（核心）
   流程：填运单号 → 逐箱录型号数量 → 汇总提交
   ========================================================= */
async function viewInbound() {
  const v = $('#view');

  // 恢复未完成的草稿
  const saved = await KV.get(DRAFT_IN_KEY);
  if (saved) {
    Draft.waybill = saved.waybill || '';
    Draft.supplier = saved.supplier || '';
    Draft.remark = saved.remark || '';
    Draft.boxes = saved.boxes || [];
  }

  renderInbound();
  v.onclick = inboundClick;

  function totalBoxes() { return Draft.boxes.length; }
  function totalQty() { return Draft.boxes.reduce((s, b) => s + b.items.reduce((x, i) => x + i.qty, 0), 0); }

  async function saveDraft() {
    await KV.set(DRAFT_IN_KEY, { ...Draft, saved_at: Date.now() });
  }

  function renderInbound() {
    const total = totalQty();
    const boxHtml = Draft.boxes.map((b, bi) => {
      const bt = b.items.reduce((x, i) => x + i.qty, 0);
      const items = b.items.length
        ? b.items.map((it) => `
            <div class="item-row">
              <div style="min-width:0;flex:1">
                <div class="item-sku">${esc(it.sku)}</div>
                ${it.name && it.name !== it.sku ? `<div class="item-name">${esc(it.name)}</div>` : ''}
              </div>
              <div class="item-qty">${it.qty}<small>件</small></div>
              <button class="icon-btn" data-del-item="${bi}|${it.key}" title="删除" style="width:32px;height:32px">
                <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
              </button>
            </div>`).join('')
        : `<div class="row-meta" style="padding:8px 0">该箱还没有明细</div>`;
      return `
        <div class="box-card">
          <div class="box-head">
            <span>第 <span class="n">${bi + 1}</span> 箱 · <span style="font-weight:400;color:var(--text-dim)">${esc(b.box_no)}</span></span>
            <span style="display:flex;align-items:center;gap:8px">
              <span style="color:var(--text-dim);font-weight:400">${bt} 件</span>
              <button class="icon-btn" data-del-box="${bi}" style="width:32px;height:32px" title="删除该箱">
                <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
              </button>
            </span>
          </div>
          <div class="box-body">${items}</div>
        </div>`;
    }).join('');

    // 各型号汇总
    const summaryMap = new Map();
    Draft.boxes.forEach((b) => b.items.forEach((i) => {
      const k = i.sku;
      const prev = summaryMap.get(k) || { sku: i.sku, name: i.name, qty: 0, boxes: 0 };
      prev.qty += i.qty;
      prev.boxes += 1;
      summaryMap.set(k, prev);
    }));
    const summary = Array.from(summaryMap.values()).sort((a, b) => a.sku.localeCompare(b.sku));

    v.innerHTML = `
      <div class="card">
        <div class="card-head"><span class="card-title">运单信息</span>
          ${Draft.waybill ? `<span class="card-sub">${totalBoxes()} 箱 · ${total} 件</span>` : ''}</div>
        <div style="display:flex;flex-direction:column;gap:11px">
          <label class="field"><span class="field-label">运单号 *</span>
            <input id="f-waybill" placeholder="如 SF1234567890123" value="${esc(Draft.waybill)}"></label>
          <label class="field"><span class="field-label">供应商</span>
            <input id="f-supplier" placeholder="选填" value="${esc(Draft.supplier)}"></label>
          <label class="field"><span class="field-label">备注</span>
            <input id="f-remark" placeholder="选填" value="${esc(Draft.remark)}"></label>
        </div>
      </div>

      ${Draft.boxes.length ? `
      <div class="card">
        <div class="card-head"><span class="card-title">箱子明细</span>
          <button class="btn btn-sm btn-primary" data-add-box>继续加一箱</button></div>
        ${boxHtml}
      </div>` : ''}

      ${summary.length ? `
      <div class="card">
        <div class="card-head"><span class="card-title">本单汇总</span>
          <span class="card-sub">${totalBoxes()} 箱 / ${total} 件</span></div>
        ${summary.map((s) => `
          <div class="item-row">
            <div style="min-width:0;flex:1">
              <div class="item-sku">${esc(s.sku)}</div>
              <div class="item-name">${esc(s.name || '')} · 分布在 ${s.boxes} 箱</div>
            </div>
            <div class="item-qty">${s.qty}<small>件</small></div>
          </div>`).join('')}
      </div>` : ''}

      ${!Draft.boxes.length ? `
      <div class="empty">
        <div class="empty-icon">📦</div>
        <div class="empty-text">先填运单号，然后点「开始点数」<br>逐箱录入每个型号的数量</div>
      </div>` : ''}

      <div class="actionbar">
        ${Draft.boxes.length
          ? `<button class="btn btn-ghost" data-clear-all>清空重录</button>
             <button class="btn btn-ok" data-submit>提交入库</button>`
          : `<button class="btn btn-primary btn-block" data-add-box>开始点数</button>`}
      </div>`;

    $('#f-waybill')?.addEventListener('input', (e) => { Draft.waybill = e.target.value; saveDraft(); });
    $('#f-supplier')?.addEventListener('input', (e) => { Draft.supplier = e.target.value; saveDraft(); });
    $('#f-remark')?.addEventListener('input', (e) => { Draft.remark = e.target.value; saveDraft(); });
  }

  async function addBox() {
    if (!Draft.waybill.trim()) { toast('请先填运单号', 'err'); $('#f-waybill')?.focus(); return; }
    Draft.boxes.push({ box_no: String(Draft.boxes.length + 1), items: [] });
    await saveDraft();
    renderInbound();
    setTimeout(() => boxSheet(Draft.boxes.length - 1, renderInbound), 60);
  }

  async function inboundClick(e) {
    const t = e.target.closest('[data-act],[data-add-box],[data-submit],[data-clear-all],[data-del-box],[data-del-item]');
    if (!t) return;

    if (t.dataset.addBox !== undefined) return addBox();

    if (t.dataset.delBox !== undefined) {
      Draft.boxes.splice(+t.dataset.delBox, 1);
      await saveDraft(); renderInbound(); return;
    }

    if (t.dataset.delItem !== undefined) {
      const [bi, key] = t.dataset.delItem.split('|');
      const b = Draft.boxes[+bi];
      if (b) { b.items = b.items.filter((i) => i.key !== key); await saveDraft(); renderInbound(); }
      return;
    }

    if (t.dataset.clearAll !== undefined) {
      if (!confirm('清空当前单的所有箱？此操作不可撤销')) return;
      Draft.boxes = []; Draft.waybill = ''; Draft.supplier = ''; Draft.remark = '';
      await KV.del(DRAFT_IN_KEY);
      renderInbound(); return;
    }

    if (t.dataset.submit !== undefined) return submitInbound();
  }

  async function submitInbound() {
    const waybill = Draft.waybill.trim();
    if (!waybill) { toast('运单号不能为空', 'err'); return; }
    if (!Draft.boxes.length) { toast('至少录入一箱', 'err'); return; }
    for (let i = 0; i < Draft.boxes.length; i++) {
      if (!Draft.boxes[i].items.length) { toast(`第 ${i + 1} 箱还没有明细`, 'err'); return; }
    }

    const payload = {
      waybill_no: waybill,
      supplier_name: Draft.supplier.trim() || null,
      remark: Draft.remark.trim() || null,
      boxes: Draft.boxes.map((b) => ({
        box_no: b.box_no,
        items: b.items.map((i) => ({ sku: i.sku, qty: i.qty })),
      })),
      op_id: uuid(),
      post: true,
    };

    const btn = $('[data-submit]');
    btn.disabled = true; btn.textContent = '提交中…';

    try {
      const r = await write('inbound_create', payload);
      if (r.queued) {
        toast('已离线保存，联网后自动入库');
      } else {
        const d = r.data;
        toast(`入库成功：${d.doc_no}`, 'ok');
        Draft.boxes = []; Draft.waybill = ''; Draft.supplier = ''; Draft.remark = '';
        await KV.del(DRAFT_IN_KEY);
        showInboundDetail(d);
        return;
      }
      Draft.boxes = []; Draft.waybill = ''; Draft.supplier = ''; Draft.remark = '';
      await KV.del(DRAFT_IN_KEY);
      go('dash');
    } catch (err) {
      toast(err.message, 'err');
      btn.disabled = false; btn.textContent = '提交入库';
    }
  }
}

/* ---------- 单箱录入弹层 ---------- */
function boxSheet(boxIndex, onDone) {
  const box = Draft.boxes[boxIndex];
  const wrap = document.createElement('div');
  wrap.className = 'sheet';
  wrap.innerHTML = `
    <div class="sheet-mask" data-x></div>
    <div class="sheet-body" style="max-height:82vh;overflow-y:auto">
      <div class="sheet-grip"></div>
      <div class="sheet-user">第 ${boxIndex + 1} 箱 · 箱号 ${esc(box.box_no)}</div>

      <div class="field" style="margin-bottom:10px">
        <span class="field-label">箱号</span>
        <input id="bx-no" value="${esc(box.box_no)}">
      </div>

      <div class="field">
        <span class="field-label">搜索型号（输入型号或名称，回车添加）</span>
        <input id="bx-search" placeholder="如 A001 或 名称关键词" autocomplete="off">
      </div>
      <div id="bx-suggest" class="card suggest hidden" style="max-height:190px;overflow-y:auto"></div>

      <div id="bx-items" style="margin:10px 0"></div>

      <div style="display:flex;gap:9px;margin-top:6px">
        <input id="bx-manual" class="input" placeholder="或直接输入新型号" style="flex:1">
        <button class="btn btn-ghost" data-x-add style="flex:none">添加</button>
      </div>

      <div class="actionbar">
        <button class="btn btn-ghost" data-x>取消</button>
        <button class="btn btn-ok" data-x-ok>完成本箱</button>
      </div>
    </div>`;

  document.body.appendChild(wrap);
  const close = () => wrap.remove();

  function renderItems() {
    const el = $('#bx-items', wrap);
    if (!box.items.length) {
      el.innerHTML = `<div class="row-meta" style="padding:10px 0">还没有明细，搜索或直接输入型号开始点数</div>`;
      return;
    }
    el.innerHTML = box.items.map((it) => `
      <div class="item-row">
        <div style="min-width:0;flex:1">
          <div class="item-sku">${esc(it.sku)}</div>
          ${it.name && it.name !== it.sku ? `<div class="item-name">${esc(it.name)}</div>` : ''}
        </div>
        <div class="stepper">
          <button data-step="${it.key}|-1">−</button>
          <input type="number" inputmode="numeric" data-qty="${it.key}" value="${it.qty}">
          <button data-step="${it.key}|1">+</button>
        </div>
        <button class="icon-btn" data-rm="${it.key}" style="width:32px;height:32px">
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
        </button>
      </div>`).join('');
  }

  function addItem(sku, name) {
    const s = String(sku || '').trim();
    if (!s) return;
    const found = box.items.find((i) => i.sku === s);
    if (found) { found.qty += 1; toast(`${s} 数量 +1`); }
    else box.items.push({ key: uuid(), sku: s, name: name || s, qty: 1 });
    renderItems();
  }

  let searchTimer, productCache = [];
  async function ensureCache() {
    if (productCache.length) return productCache;
    try { productCache = await api('/api/products?limit=1000') || []; }
    catch (_) { productCache = []; }
    return productCache;
  }

  const searchEl = $('#bx-search', wrap);
  searchEl.addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(async () => {
      const kw = searchEl.value.trim();
      const box2 = $('#bx-suggest', wrap);
      if (!kw) { box2.classList.add('hidden'); return; }
      const list = await ensureCache();
      const k = kw.toLowerCase();
      const hit = list.filter((p) => p.sku.toLowerCase().includes(k) || (p.name || '').toLowerCase().includes(k)).slice(0, 12);
      if (!hit.length) {
        box2.classList.remove('hidden');
        box2.innerHTML = `<div class="suggest-item" data-new="${esc(kw)}">型号「${esc(kw)}」不存在 · <b>点击新建并添加</b></div>`;
        return;
      }
      box2.classList.remove('hidden');
      box2.innerHTML = hit.map((p) => `
        <div class="suggest-item" data-pick="${esc(p.sku)}" data-name="${esc(p.name)}">
          <span>${esc(p.sku)}</span>
          <span style="color:var(--text-dim);font-size:12.5px">${esc(p.name)}</span>
          <span class="stock">库存 ${p.stock_qty}</span>
        </div>`).join('');
    }, 180);
  });

  wrap.addEventListener('click', (e) => {
    const t = e.target.closest('[data-x],[data-x-add],[data-x-ok],[data-pick],[data-new],[data-step],[data-rm]');
    if (!t) return;

    if (t.hasAttribute('data-x')) return close();

    if (t.hasAttribute('data-x-add') || t.hasAttribute('data-new')) {
      const v = t.hasAttribute('data-new') ? t.dataset.new : $('#bx-manual', wrap).value;
      addItem(v, v);
      if (!t.hasAttribute('data-new')) $('#bx-manual', wrap).value = '';
      $('#bx-suggest', wrap).classList.add('hidden');
      return;
    }

    if (t.dataset.pick) {
      addItem(t.dataset.pick, t.dataset.name);
      searchEl.value = '';
      $('#bx-suggest', wrap).classList.add('hidden');
      return;
    }

    if (t.dataset.step) {
      const [key, delta] = t.dataset.step.split('|');
      const it = box.items.find((i) => i.key === key);
      if (it) it.qty = Math.max(1, it.qty + Number(delta));
      renderItems();
      return;
    }

    if (t.dataset.rm) {
      box.items = box.items.filter((i) => i.key !== t.dataset.rm);
      renderItems();
      return;
    }

    if (t.hasAttribute('data-x-ok')) {
      if (!box.items.length) { toast('本箱还没有明细', 'err'); return; }
      $('#bx-no', wrap).value = $('#bx-no', wrap).value.trim() || box.box_no;
      box.box_no = $('#bx-no', wrap).value;
      close();
      onDone && onDone();
      return;
    }
  });

  wrap.addEventListener('input', (e) => {
    const q = e.target.dataset && e.target.dataset.qty;
    if (q) {
      const it = box.items.find((i) => i.key === q);
      if (it) it.qty = Math.max(0, parseInt(e.target.value || '0', 10) || 0);
    }
  });

  wrap.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.target === searchEl) {
      e.preventDefault();
      const v = searchEl.value.trim();
      if (v) { addItem(v, v); searchEl.value = ''; $('#bx-suggest', wrap).classList.add('hidden'); }
    }
  });

  renderItems();
  setTimeout(() => searchEl.focus(), 80);
}

/* =========================================================
   入库单详情
   ========================================================= */
function showInboundDetail(d) {
  const wrap = document.createElement('div');
  wrap.className = 'sheet';
  wrap.innerHTML = `
    <div class="sheet-mask" data-x></div>
    <div class="sheet-body" style="max-height:86vh;overflow-y:auto">
      <div class="sheet-grip"></div>
      <div class="sheet-user">${esc(d.doc_no)}
        <span class="tag ${d.status === 'posted' ? 'tag-ok' : 'tag-draft'}">${d.status === 'posted' ? '已入库' : '草稿'}</span>
      </div>
      <div class="row-meta" style="margin-bottom:12px">
        运单号 ${esc(d.waybill_no)}${d.supplier_name ? ' · ' + esc(d.supplier_name) : ''}<br>
        ${d.total_boxes} 箱 / 共 ${d.total_qty} 件 · 经手 ${esc(d.creator_name)}
      </div>
      <div class="section-label">每箱型号明细</div>
      ${d.boxes.map((b, i) => `
        <div class="box-card">
          <div class="box-head">
            <span>第 ${i + 1} 箱 · ${esc(b.box_no)}</span>
            <span style="color:var(--text-dim);font-weight:400">${b.total_qty} 件</span>
          </div>
          <div class="box-body">
            ${b.items.map((it) => `
              <div class="item-row">
                <div style="min-width:0;flex:1">
                  <div class="item-sku">${esc(it.sku)}</div>
                  <div class="item-name">${esc(it.name)}</div>
                </div>
                <div class="item-qty">${it.qty}<small>${esc(it.unit)}</small></div>
              </div>`).join('')}
          </div>
        </div>`).join('')}
      <div class="actionbar"><button class="btn btn-primary" data-x>知道了</button></div>
    </div>`;
  document.body.appendChild(wrap);
  wrap.addEventListener('click', (e) => { if (e.target.closest('[data-x]')) wrap.remove(); });
}

/* =========================================================
   视图：出库
   ========================================================= */
async function viewOutbound() {
  const v = $('#view');
  const state = { customer: '', remark: '', items: [], products: [] };

  try { state.products = await api('/api/products?limit=1000') || []; }
  catch (_) { state.products = []; }

  function render() {
    const total = state.items.reduce((s, i) => s + i.qty, 0);
    v.innerHTML = `
      <div class="card">
        <div class="card-head"><span class="card-title">出货信息</span></div>
        <div style="display:flex;flex-direction:column;gap:11px">
          <label class="field"><span class="field-label">客户名 *</span>
            <input id="o-customer" placeholder="如 某某商行" value="${esc(state.customer)}"></label>
          <label class="field"><span class="field-label">备注</span>
            <input id="o-remark" placeholder="选填" value="${esc(state.remark)}"></label>
        </div>
      </div>

      <div class="card">
        <div class="card-head"><span class="card-title">出货明细</span>
          <span class="card-sub">${state.items.length} 种 / ${total} 件</span></div>

        <div class="field" style="margin-bottom:10px">
          <input id="o-search" placeholder="搜索型号添加" autocomplete="off">
        </div>
        <div id="o-suggest" class="suggest hidden" style="max-height:200px;overflow-y:auto;margin-bottom:8px"></div>

        ${state.items.length ? state.items.map((it) => `
          <div class="item-row">
            <div style="min-width:0;flex:1">
              <div class="item-sku">${esc(it.sku)}</div>
              <div class="item-name">${esc(it.name)} · 库存 ${it.stock}</div>
            </div>
            <div class="stepper">
              <button data-ost="${it.sku}|-1">−</button>
              <input type="number" inputmode="numeric" data-oqty="${it.sku}" value="${it.qty}">
              <button data-ost="${it.sku}|1">+</button>
            </div>
            <button class="icon-btn" data-orm="${it.sku}" style="width:32px;height:32px">
              <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
            </button>
          </div>`).join('') : `<div class="row-meta" style="padding:10px 0">还没有明细</div>`}
      </div>

      <div class="actionbar">
        <button class="btn btn-ok" data-o-submit ${state.items.length ? '' : 'disabled'}>提交出库</button>
      </div>`;

    $('#o-customer').oninput = (e) => { state.customer = e.target.value; };
    $('#o-remark').oninput = (e) => { state.remark = e.target.value; };

    let t2;
    $('#o-search').oninput = (e) => {
      clearTimeout(t2);
      const kw = e.target.value.trim().toLowerCase();
      const box = $('#o-suggest');
      if (!kw) { box.classList.add('hidden'); return; }
      t2 = setTimeout(() => {
        const hit = state.products
          .filter((p) => p.sku.toLowerCase().includes(kw) || (p.name || '').toLowerCase().includes(kw))
          .slice(0, 12);
        if (!hit.length) { box.classList.add('hidden'); return; }
        box.classList.remove('hidden');
        box.innerHTML = hit.map((p) => `
          <div class="suggest-item" data-opick="${esc(p.sku)}">
            <span>${esc(p.sku)}</span>
            <span style="color:var(--text-dim);font-size:12.5px">${esc(p.name)}</span>
            <span class="stock">库存 ${p.stock_qty}</span>
          </div>`).join('');
      }, 160);
    };
  }

  render();

  v.onclick = async (e) => {
    const pick = e.target.closest('[data-opick]');
    if (pick) {
      const p = state.products.find((x) => x.sku === pick.dataset.opick);
      const found = state.items.find((i) => i.sku === p.sku);
      if (found) found.qty += 1;
      else state.items.push({ sku: p.sku, name: p.name, qty: 1, stock: p.stock_qty });
      $('#o-suggest').classList.add('hidden');
      $('#o-search').value = '';
      render();
      return;
    }

    const st = e.target.closest('[data-ost]');
    if (st) {
      const [sku, d] = st.dataset.ost.split('|');
      const it = state.items.find((i) => i.sku === sku);
      if (it) it.qty = Math.max(1, it.qty + Number(d));
      render();
      return;
    }

    const rm = e.target.closest('[data-orm]');
    if (rm) {
      state.items = state.items.filter((i) => i.sku !== rm.dataset.orm);
      render();
      return;
    }

    if (e.target.closest('[data-o-submit]')) {
      if (!state.customer.trim()) { toast('请填客户名', 'err'); return; }
      const btn = e.target.closest('[data-o-submit]');
      btn.disabled = true; btn.textContent = '提交中…';
      try {
        const r = await write('outbound_create', {
          customer: state.customer.trim(),
          remark: state.remark.trim() || null,
          items: state.items.map((i) => ({ sku: i.sku, qty: i.qty })),
          op_id: uuid(),
          post: true,
        });
        if (r.queued) toast('已离线保存，联网后自动出库');
        else toast(`出库成功：${r.data.doc_no}`, 'ok');
        go('dash');
      } catch (err) {
        toast(err.message, 'err');
        btn.disabled = false; btn.textContent = '提交出库';
      }
    }
  };

  v.oninput = (e) => {
    const q = e.target.dataset && e.target.dataset.oqty;
    if (q) {
      const it = state.items.find((i) => i.sku === q);
      if (it) it.qty = Math.max(0, parseInt(e.target.value || '0', 10) || 0);
    }
  };
}

/* =========================================================
   视图：库存
   ========================================================= */
async function viewStock() {
  const v = $('#view');
  const rows = await api('/api/stock?limit=500') || [];
  const kw = (v.dataset.kw || '').toLowerCase();
  const list = kw ? rows.filter((r) => r.sku.toLowerCase().includes(kw) || (r.name || '').toLowerCase().includes(kw)) : rows;
  const low = list.filter((r) => r.warn_qty > 0 && r.stock_qty <= r.warn_qty);

  v.innerHTML = `
    <div class="searchbar"><input id="st-kw" placeholder="搜索型号或名称" value="${esc(v.dataset.kw || '')}"></div>
    ${low.length ? `<div class="card" style="border-color:rgba(229,72,77,.4)">
      <div class="card-head"><span class="card-title" style="color:var(--danger)">低库存预警 ${low.length} 种</span></div>
      <div style="display:flex;flex-wrap:wrap;gap:7px">
        ${low.map((r) => `<span class="tag tag-danger">${esc(r.sku)} ${r.stock_qty}</span>`).join('')}
      </div>
    </div>` : ''}
    <div class="list">
      ${list.length ? list.map((r) => `
        <div class="row" data-sku="${esc(r.sku)}">
          <div class="row-main">
            <div style="min-width:0">
              <div class="row-title">${esc(r.sku)}</div>
              <div class="row-meta">${esc(r.name)}${r.spec ? ' · ' + esc(r.spec) : ''}</div>
            </div>
            <div style="text-align:right;flex:none">
              <div style="font-size:19px;font-weight:700;font-variant-numeric:tabular-nums;
                color:${r.warn_qty > 0 && r.stock_qty <= r.warn_qty ? 'var(--danger)' : 'var(--text)'}">
                ${num(r.stock_qty)}<span class="stat-unit">${esc(r.unit)}</span>
              </div>
              ${r.warn_qty > 0 ? `<div class="row-sub">预警 ${r.warn_qty}</div>` : ''}
            </div>
          </div>
        </div>`).join('') : `<div class="empty"><div class="empty-icon">📦</div><div class="empty-text">没有匹配的型号</div></div>`}
    </div>`;

  const kwEl = $('#st-kw');
  let tm;
  kwEl.oninput = () => {
    clearTimeout(tm);
    tm = setTimeout(() => { v.dataset.kw = kwEl.value; viewStock(); }, 300);
  };
  v.onclick = (e) => {
    const row = e.target.closest('[data-sku]');
    if (row) showMovements(row.dataset.sku);
  };
}

async function showMovements(sku) {
  const products = await api('/api/products?limit=1000') || [];
  const p = products.find((x) => x.sku === sku);
  if (!p) { toast('未找到型号', 'err'); return; }
  const list = await api(`/api/stock/${p.id}/movements?limit=60`) || [];

  const wrap = document.createElement('div');
  wrap.className = 'sheet';
  wrap.innerHTML = `
    <div class="sheet-mask" data-x></div>
    <div class="sheet-body" style="max-height:82vh;overflow-y:auto">
      <div class="sheet-grip"></div>
      <div class="sheet-user">${esc(p.sku)} · 当前 ${p.stock_qty} ${esc(p.unit)}</div>
      <div class="section-label">出入库流水</div>
      ${list.length ? list.map((m) => `
        <div class="item-row">
          <div style="min-width:0;flex:1">
            <div class="item-sku" style="font-size:13.5px">
              <span class="tag ${m.change_qty > 0 ? 'tag-ok' : 'tag-draft'}">${m.change_qty > 0 ? '入' : '出'}</span>
              ${esc(m.biz_no || '')}
            </div>
            <div class="item-name">${shortTime(m.created_at)} · ${esc(m.operator_name)}</div>
          </div>
          <div class="item-qty" style="color:${m.change_qty > 0 ? 'var(--ok)' : 'var(--primary)'}">
            ${m.change_qty > 0 ? '+' : ''}${m.change_qty}
          </div>
          <div style="font-size:11.5px;color:var(--text-mute);min-width:52px;text-align:right">
            余 ${m.balance_qty}
          </div>
        </div>`).join('') : `<div class="row-meta">暂无流水</div>`}
      <div class="actionbar"><button class="btn btn-primary" data-x>关闭</button></div>
    </div>`;
  document.body.appendChild(wrap);
  wrap.addEventListener('click', (e) => { if (e.target.closest('[data-x]')) wrap.remove(); });
}

/* =========================================================
   视图：型号管理
   ========================================================= */
async function viewProducts() {
  const v = $('#view');
  const list = await api('/api/products?limit=1000') || [];
  const user = JSON.parse(localStorage.getItem(USER_KEY) || '{}');
  const isAdmin = user.role === 'admin';

  v.innerHTML = `
    <div class="searchbar"><input id="p-kw" placeholder="搜索型号或名称"></div>
    <div class="actionbar" style="position:static;padding-bottom:12px">
      <button class="btn btn-primary btn-block" data-p-new>新增型号</button>
    </div>
    <div class="list" id="p-list"></div>`;

  function paint(kw) {
    const k = (kw || '').toLowerCase();
    const rows = list.filter((p) => !k || p.sku.toLowerCase().includes(k) || (p.name || '').toLowerCase().includes(k));
    $('#p-list').innerHTML = rows.length ? rows.map((p) => `
      <div class="row" data-p-id="${p.id}">
        <div class="row-main">
          <div style="min-width:0">
            <div class="row-title">${esc(p.sku)}${p.active ? '' : ' <span class="tag tag-void">已停用</span>'}</div>
            <div class="row-meta">${esc(p.name)} · 库存 ${p.stock_qty}${p.warn_qty ? ' · 预警 ' + p.warn_qty : ''}</div>
          </div>
          <button class="btn btn-sm btn-ghost" data-p-edit="${p.id}">编辑</button>
        </div>
      </div>`).join('') : `<div class="empty"><div class="empty-icon">🏷️</div><div class="empty-text">还没有型号，点上方新增</div></div>`;
  }

  paint('');
  let tm;
  $('#p-kw').oninput = (e) => { clearTimeout(tm); tm = setTimeout(() => paint(e.target.value), 250); };

  v.onclick = async (e) => {
    if (e.target.closest('[data-p-new]')) return productSheet(null, list, isAdmin, () => viewProducts());
    const ed = e.target.closest('[data-p-edit]');
    if (ed) {
      const p = list.find((x) => x.id === +ed.dataset.pEdit);
      return productSheet(p, list, isAdmin, () => viewProducts());
    }
  };
}

function productSheet(p, list, isAdmin, onDone) {
  const edit = !!p;
  const wrap = document.createElement('div');
  wrap.className = 'sheet';
  wrap.innerHTML = `
    <div class="sheet-mask" data-x></div>
    <div class="sheet-body" style="max-height:86vh;overflow-y:auto">
      <div class="sheet-grip"></div>
      <div class="sheet-user">${edit ? '编辑型号' : '新增型号'}</div>
      <div style="display:flex;flex-direction:column;gap:11px">
        <label class="field"><span class="field-label">型号 *</span>
          <input id="m-sku" value="${esc(p?.sku || '')}" ${edit ? '' : 'placeholder="如 A001"'}></label>
        <label class="field"><span class="field-label">名称 *</span>
          <input id="m-name" value="${esc(p?.name || '')}" placeholder="商品名称"></label>
        <label class="field"><span class="field-label">规格</span>
          <input id="m-spec" value="${esc(p?.spec || '')}" placeholder="如 红色 / XL"></label>
        <label class="field"><span class="field-label">单位</span>
          <input id="m-unit" value="${esc(p?.unit || '件')}"></label>
        <div style="display:grid;grid-template-columns:1fr 1fr;gap:10px">
          <label class="field"><span class="field-label">成本价</span>
            <input id="m-cost" type="number" inputmode="decimal" step="0.01" value="${p?.cost_price ?? 0}"></label>
          <label class="field"><span class="field-label">销售价</span>
            <input id="m-sale" type="number" inputmode="decimal" step="0.01" value="${p?.sale_price ?? 0}"></label>
        </div>
        <label class="field"><span class="field-label">低库存预警（0=不预警）</span>
          <input id="m-warn" type="number" inputmode="numeric" value="${p?.warn_qty ?? 0}"></label>
      </div>
      <div class="actionbar">
        ${edit && isAdmin ? `<button class="btn btn-ghost" data-m-del style="flex:0 0 auto">删除</button>` : ''}
        <button class="btn btn-ghost" data-x>取消</button>
        <button class="btn btn-ok" data-m-ok>保存</button>
      </div>
    </div>`;

  document.body.appendChild(wrap);
  const close = () => wrap.remove();

  wrap.addEventListener('click', async (e) => {
    if (e.target.closest('[data-x]')) return close();

    if (e.target.closest('[data-m-ok]')) {
      const body = {
        id: edit ? p.id : undefined,
        sku: $('#m-sku').value.trim(),
        name: $('#m-name').value.trim(),
        spec: $('#m-spec').value.trim() || null,
        unit: $('#m-unit').value.trim() || '件',
        cost_price: parseFloat($('#m-cost').value || '0') || 0,
        sale_price: parseFloat($('#m-sale').value || '0') || 0,
        warn_qty: parseInt($('#m-warn').value || '0', 10) || 0,
      };
      if (!body.sku || !body.name) { toast('型号和名称必填', 'err'); return; }
      try {
        await api('/api/products', { method: 'POST', body });
        toast('已保存', 'ok');
        close();
        onDone && onDone();
      } catch (err) { toast(err.message, 'err'); }
      return;
    }

    if (e.target.closest('[data-m-del]')) {
      if (!confirm(`确定删除型号「${p.sku}」？有历史流水的会自动停用`)) return;
      try {
        const r = await api('/api/products/' + p.id, { method: 'DELETE' });
        toast(r.action === 'disabled' ? '有历史流水，已停用' : '已删除', 'ok');
        close();
        onDone && onDone();
      } catch (err) { toast(err.message, 'err'); }
    }
  });
}

/* =========================================================
   视图：单据列表
   ========================================================= */
async function viewInboundList() {
  const v = $('#view');
  const kw = v.dataset.kw || '';
  const list = await api(`/api/inbound?limit=100&keyword=${encodeURIComponent(kw)}`) || [];

  v.innerHTML = `
    <div class="searchbar"><input id="i-kw" placeholder="搜索运单号 / 单号 / 供应商" value="${esc(kw)}"></div>
    <div class="list">
      ${list.length ? list.map((o) => `
        <div class="row" data-in-id="${o.id}">
          <div class="row-main">
            <div style="min-width:0">
              <div class="row-title">${esc(o.waybill_no)}</div>
              <div class="row-meta">${esc(o.doc_no)}${o.supplier_name ? ' · ' + esc(o.supplier_name) : ''}</div>
              <div class="row-sub">${o.total_boxes} 箱 / ${o.total_qty} 件 · ${shortTime(o.created_at)}</div>
            </div>
            <span class="tag ${o.status === 'posted' ? 'tag-ok' : 'tag-draft'}">${o.status === 'posted' ? '已入库' : '草稿'}</span>
          </div>
        </div>`).join('') : `<div class="empty"><div class="empty-icon">📋</div><div class="empty-text">暂无入库单</div></div>`}
    </div>`;

  let tm;
  $('#i-kw').oninput = (e) => { clearTimeout(tm); tm = setTimeout(() => { v.dataset.kw = e.target.value; viewInboundList(); }, 300); };
  v.onclick = async (e) => {
    const row = e.target.closest('[data-in-id]');
    if (row) {
      try { showInboundDetail(await api('/api/inbound/' + row.dataset.inId)); }
      catch (err) { toast(err.message, 'err'); }
    }
  };
}

async function viewOutboundList() {
  const v = $('#view');
  const kw = v.dataset.kw || '';
  const list = await api(`/api/outbound?limit=100&keyword=${encodeURIComponent(kw)}`) || [];

  v.innerHTML = `
    <div class="searchbar"><input id="ob-kw" placeholder="搜索客户 / 单号" value="${esc(kw)}"></div>
    <div class="list">
      ${list.length ? list.map((o) => `
        <div class="row" data-ob-id="${o.id}">
          <div class="row-main">
            <div style="min-width:0">
              <div class="row-title">${esc(o.customer)}</div>
              <div class="row-meta">${esc(o.doc_no)}</div>
              <div class="row-sub">${o.total_qty} 件 · ${shortTime(o.created_at)}</div>
            </div>
            <span class="tag ${o.status === 'posted' ? 'tag-ok' : 'tag-draft'}">${o.status === 'posted' ? '已出库' : '草稿'}</span>
          </div>
        </div>`).join('') : `<div class="empty"><div class="empty-icon">📋</div><div class="empty-text">暂未出库单</div></div>`}
    </div>`;

  let tm;
  $('#ob-kw').oninput = (e) => { clearTimeout(tm); tm = setTimeout(() => { v.dataset.kw = e.target.value; viewOutboundList(); }, 300); };
  v.onclick = async (e) => {
    const row = e.target.closest('[data-ob-id]');
    if (!row) return;
    try {
      const d = await api('/api/outbound/' + row.dataset.obId);
      const wrap = document.createElement('div');
      wrap.className = 'sheet';
      wrap.innerHTML = `
        <div class="sheet-mask" data-x></div>
        <div class="sheet-body" style="max-height:82vh;overflow-y:auto">
          <div class="sheet-grip"></div>
          <div class="sheet-user">${esc(d.doc_no)} · ${esc(d.customer)}</div>
          <div class="row-meta" style="margin-bottom:12px">${d.total_qty} 件 · ${shortTime(d.created_at)} · ${esc(d.creator_name)}</div>
          ${d.items.map((it) => `
            <div class="item-row">
              <div style="min-width:0;flex:1">
                <div class="item-sku">${esc(it.sku)}</div>
                <div class="item-name">${esc(it.name)}</div>
              </div>
              <div class="item-qty">${it.qty}<small>${esc(it.unit)}</small></div>
            </div>`).join('')}
          <div class="actionbar"><button class="btn btn-primary" data-x>关闭</button></div>
        </div>`;
      document.body.appendChild(wrap);
      wrap.addEventListener('click', (e2) => { if (e2.target.closest('[data-x]')) wrap.remove(); });
    } catch (err) { toast(err.message, 'err'); }
  };
}

/* =========================================================
   视图：待同步队列
   ========================================================= */
async function viewPending() {
  const v = $('#view');
  const ops = await Queue.all();
  const errs = (await KV.get('last_sync_errors')) || [];
  const errMap = new Map(errs.map((e) => [e.op_id, e.message]));

  const typeName = { inbound_create: '入库', outbound_create: '出库' };

  v.innerHTML = `
    <div class="card">
      <div class="card-head"><span class="card-title">离线队列</span>
        <span class="card-sub">${ops.length} 条待上传</span></div>
      ${ops.length ? `<button class="btn btn-primary btn-block" data-p-sync">立即同步</button>` : ''}
    </div>
    <div class="list">
      ${ops.length ? ops.map((o) => `
        <div class="row">
          <div class="row-main">
            <div style="min-width:0">
              <div class="row-title">
                <span class="tag ${o.op_type === 'inbound_create' ? 'tag-ok' : 'tag-draft'}">${typeName[o.op_type] || o.op_type}</span>
                ${esc(o.payload.waybill_no || o.payload.customer || '')}
              </div>
              <div class="row-meta">${shortTime(new Date(o.seq).toISOString())} · ${o.payload.boxes ? o.payload.boxes.length + ' 箱' : (o.payload.items?.length || 0) + ' 种'}</div>
              ${errMap.has(o.op_id) ? `<div class="row-sub" style="color:var(--danger)">⚠ ${esc(errMap.get(o.op_id))}</div>` : ''}
            </div>
            <button class="btn btn-sm btn-ghost" data-p-del="${o.op_id}">删除</button>
          </div>
        </div>`).join('') : `<div class="empty"><div class="empty-icon">✅</div><div class="empty-text">队列已清空，全部数据已同步</div></div>`}
    </div>`;

  v.onclick = async (e) => {
    if (e.target.closest('[data-p-sync]')) { await syncNow(); viewPending(); return; }
    const d = e.target.closest('[data-p-del]');
    if (d) {
      if (!confirm('删除这条未同步的记录？该操作不会影响服务器数据')) return;
      await Queue.remove(d.dataset.pDel);
      await refreshBadge();
      viewPending();
    }
  };
}

/* =========================================================
   视图：修改密码
   ========================================================= */
async function viewPassword() {
  const v = $('#view');
  v.innerHTML = `
    <div class="card">
      <div class="card-head"><span class="card-title">修改密码</span></div>
      <div style="display:flex;flex-direction:column;gap:11px">
        <label class="field"><span class="field-label">原密码</span>
          <input id="pw-old" type="password" autocomplete="current-password"></label>
        <label class="field"><span class="field-label">新密码（至少 6 位）</span>
          <input id="pw-new" type="password" autocomplete="new-password"></label>
        <label class="field"><span class="field-label">确认新密码</span>
          <input id="pw-new2" type="password" autocomplete="new-password"></label>
      </div>
      <div class="actionbar"><button class="btn btn-ok" data-pw-ok>保存</button></div>
    </div>`;

  v.onclick = async (e) => {
    if (!e.target.closest('[data-pw-ok]')) return;
    const o = $('#pw-old').value, n = $('#pw-new').value, n2 = $('#pw-new2').value;
    if (n !== n2) { toast('两次输入的新密码不一致', 'err'); return; }
    try {
      await api('/api/me/password', { method: 'PUT', body: { old_password: o, new_password: n } });
      toast('密码已修改', 'ok');
      go('dash');
    } catch (err) { toast(err.message, 'err'); }
  };
}

/* =========================================================
   启动
   ========================================================= */
$('#login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const btn = $('#login-btn');
  const err = $('#login-error');
  err.classList.add('hidden');
  btn.disabled = true; btn.textContent = '登录中…';
  try {
    const d = await api('/api/auth/login', {
      method: 'POST',
      body: { username: $('#login-user').value.trim(), password: $('#login-pass').value },
    });
    localStorage.setItem(TOKEN_KEY, d.token);
    localStorage.setItem(USER_KEY, JSON.stringify(d.user));
    toast('欢迎，' + d.user.name, 'ok');
    enterApp();
  } catch (e2) {
    err.textContent = e2.message;
    err.classList.remove('hidden');
  } finally {
    btn.disabled = false; btn.textContent = '登 录';
  }
});

function enterApp() {
  switchPage('app');
  refreshBadge();
  go('dash');
  syncNow(true);
}

$('#menu-btn').addEventListener('click', openSheet);
$('#sync-btn').addEventListener('click', async () => {
  const n = await Queue.size();
  if (!n) { toast('没有待同步的数据'); return; }
  await syncNow();
});
$('#sheet').addEventListener('click', (e) => {
  if (e.target.hasAttribute('data-close-sheet')) return closeSheet();
  const item = e.target.closest('[data-go]');
  if (!item) return;
  const a = item.dataset.go;
  if (a === 'logout') {
    if (!confirm('退出登录？离线队列会保留在本机。')) return;
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USER_KEY);
    closeSheet();
    switchPage('login');
    return;
  }
  go(a);
});

$$('.tab').forEach((t) => t.addEventListener('click', () => go(t.dataset.tab)));

window.addEventListener('online', () => { updateNet(); });
window.addEventListener('offline', updateNet);
window.addEventListener('visibilitychange', () => {
  if (!document.hidden && navigator.onLine) syncNow(true);
});

(async function boot() {
  updateNet();
  if (localStorage.getItem(TOKEN_KEY)) {
    try { await api('/api/me'); enterApp(); }
    catch (_) { switchPage('login'); }
  } else {
    switchPage('login');
  }
  // 后台轮询同步：手机锁屏再打开也能补上
  setInterval(() => { if (navigator.onLine && !document.hidden) syncNow(true); }, 45000);
})();

/* ---------- Service Worker 注册 ---------- */
if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => { /* 注册失败不影响使用 */ });
  });
}
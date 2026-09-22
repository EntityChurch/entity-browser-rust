// vm-sdk.js -- what every v86 app shares (window.EntityVm). DESIGN-2026-09-14 §4.
//
// THE RULE THIS FILE WAS CUT BY: a piece moves here only when a SECOND machine
// needed it. Alpine had all of it; KolibriOS needed the same pieces and nothing
// about 9p, apk or a shell agent. So what is here is proven shared, not guessed.
//
//   host bridge     ready-for-init / init, the fallback, one handler per message
//   assets          x-asset bundles from the host, accounted in __m1.assets
//   workspace       x-work-* requests (what a machine keeps is the app's decision)
//   files           x-file out (answered by name) and a download when unhosted
//   status          the one-line status bar, download progress
//   snapshot        identity hashing and the fit check over NAMED ids
//   power           restart / restart showing the boot / turn off / start, across a reload
//   about           ⓘ: what this machine is made of, licences, and where the source is
//   clockShim       sub-millisecond performance.now on an origin that rounds it (§1.1)
//   reportStats     x-stats: busy time, instructions/s and memory, for the host's System Monitor
//   reportView      x-view: the guest screen's size, so the host can size its window to show all of it
//   makeTar         a ustar archive
//
// The page keeps: how its guest boots, how files reach the guest, what "saved"
// means. Diagnostics stay in window.__m1, where the probes already read them.
(function () {
  'use strict';
  const HOST = 'entity-host', APP = 'entity-app';
  const hosted = parent !== window;
  const $ = id => document.getElementById(id);
  const t0 = performance.now();
  const since = () => Math.round(performance.now() - t0);
  const token = () => Array.from(crypto.getRandomValues(new Uint8Array(4)), b => b.toString(16).padStart(2, '0')).join('');
  const mib = n => (n / 1048576).toFixed(1);
  const kb = n => n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`;
  const strip = s => s.replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, '');
  // A buffer handed to v86 is kept by it; give it one that is exactly the file.
  const exactBuffer = u8 => (u8.byteOffset === 0 && u8.byteLength === u8.buffer.byteLength) ? u8.buffer : u8.slice().buffer;

  const m1 = window.__m1 = window.__m1 || {};
  Object.assign(m1, {
    error: m1.error || null,
    assets: { requests: 0, bytes: 0, refused: 0, lastRefusal: null, mode: 'url', byBundle: {} },
    env: { crossOriginIsolated: (typeof crossOriginIsolated !== 'undefined') ? crossOriginIsolated : 'undefined',
           SharedArrayBuffer_present: (typeof SharedArrayBuffer !== 'undefined'),
           sandboxed: window.origin === 'null', ua: navigator.userAgent.slice(0, 90) },
  });

  // ── the bridge ──────────────────────────────────────────────────────────────
  const send = (type, x, transfer) => hosted &&
    parent.postMessage(Object.assign({ source: APP, type }, x || {}), '*', transfer || []);
  const handlers = new Map();
  const on = (type, fn) => { handlers.set(type, fn); };
  addEventListener('message', async e => {
    const m = e.data;
    if (!m || m.source !== HOST) return;
    const h = handlers.get(m.type);
    if (!h) return;
    try { await h(m); } catch (err) { send('error', { message: String(err).slice(0, 300) }); }
  });

  // start(): say ready-for-init and resolve ONCE, with what the host offers or with
  // nothing when no host answers. The contract's fallback is 150 ms; EMBEDDED it is
  // two seconds, because a frame that boots in URL mode fetches from the HOST's
  // origin, where none of the files exist -- a slow init must not lose the assets.
  // `needs`: the bundles without which host mode is pointless (else URL mode).
  let started = null;
  function start(opts) {
    if (started) return started;
    opts = opts || {};
    const needs = opts.needs || [];
    started = new Promise(resolve => {
      let done = false;
      const finish = info => { if (done) return; done = true; resolve(info); };
      on('init', m => {
        const offered = Array.isArray(m['x-assets']) ? m['x-assets'] : [];
        const bundles = needs.every(b => offered.includes(b)) && offered.length ? new Set(offered) : null;
        if (bundles) m1.assets.mode = 'host';
        finish({ hosted: true, init: m, bundles, workspace: m['x-workspace'] === true });
      });
      send('ready-for-init');
      setTimeout(() => finish({ hosted, init: null, bundles: null, workspace: false }), hosted ? 2000 : 150);
    });
    return started;
  }

  // ── assets ──────────────────────────────────────────────────────────────────
  let assetSeq = 0;
  const assetWaiting = new Map();
  function asset(bundle, key) {
    return new Promise((resolve, reject) => {
      const id = 'a' + (++assetSeq);
      assetWaiting.set(id, { resolve, reject, bundle, key });
      m1.assets.requests++;
      const per = m1.assets.byBundle[bundle] = m1.assets.byBundle[bundle] || { requests: 0, bytes: 0 };
      per.requests++;
      send('x-asset-get', { id, bundle, key });
    });
  }
  on('x-asset', m => {
    const w = assetWaiting.get(m.id);
    if (!w) return;
    assetWaiting.delete(m.id);
    if (m.ok && m.data) {
      const bytes = new Uint8Array(m.data);
      m1.assets.bytes += bytes.byteLength;
      m1.assets.byBundle[w.bundle].bytes += bytes.byteLength;
      w.resolve(bytes);
    } else {
      m1.assets.refused++;
      m1.assets.lastRefusal = `${w.bundle}/${w.key}: ${m.reason}`;
      w.reject(new Error(`the host did not serve ${w.bundle}/${w.key} (${m.reason})`));
    }
  });
  // By URL, for a page with no host. A 404 is an error here; wrap it where absence is ordinary.
  async function fetchBytes(url) {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
    return new Uint8Array(await r.arrayBuffer());
  }

  // ── workspace requests ──────────────────────────────────────────────────────
  const workWaiting = new Map();
  let workSeq = 0;
  function work(type, body, ms) {
    return new Promise((resolve, reject) => {
      const id = 'w' + (++workSeq);
      const timer = setTimeout(() => { workWaiting.delete(id); reject(new Error(`${type}: no answer from the host`)); }, ms || 60000);
      workWaiting.set(id, m => { clearTimeout(timer); resolve(m); });
      const transfer = (body.put || []).map(p => p.data).filter(Boolean);
      parent.postMessage(Object.assign({ source: APP, type, id }, body), '*', transfer);
    });
  }
  const onWork = m => { const w = workWaiting.get(m.id); if (w) { workWaiting.delete(m.id); w(m); } };
  on('x-work-listing', onWork); on('x-work-file', onWork); on('x-work-saved', onWork);

  // ── files out ───────────────────────────────────────────────────────────────
  function download(name, bytes) {
    const url = URL.createObjectURL(new Blob([bytes]));
    const a = document.createElement('a');
    a.href = url; a.download = name; document.body.appendChild(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 10000);
  }
  // `x-file-result` carries the name, not a request id, so the oldest waiting send
  // of that name is the one answered.
  const sendsWaiting = [];
  function sendFile(name, bytes, mediaType) {
    const exact = bytes.slice();      // a view into a larger buffer posts its neighbours too
    m1.files = m1.files || { sent: [], received: [], results: [] };
    m1.files.sent.push({ name, length: exact.byteLength });
    if (!hosted) { download(name, exact); return Promise.resolve({ ok: true, name, downloaded: true, detail: `${name} downloaded by the browser (no host to keep it)` }); }
    return new Promise(resolve => {
      sendsWaiting.push({ name, resolve });
      send('x-file', { name, media_type: mediaType || 'application/octet-stream', data: exact });
    });
  }
  on('x-file-result', m => {
    m1.files = m1.files || { sent: [], received: [], results: [] };
    m1.files.results.push({ ok: !!m.ok, name: m.name, reason: m.reason || null });
    const i = sendsWaiting.findIndex(s => s.name === m.name);
    const r = { ok: !!m.ok, name: m.name, reason: m.reason || null,
                detail: m.detail || (m.ok ? `the host kept ${m.name}` : `the host did not keep ${m.name}`) };
    if (i >= 0) sendsWaiting.splice(i, 1)[0].resolve(r);
    else if (api.onUnmatchedFileResult) api.onUnmatchedFileResult(r);
  });
  const requestFile = () => send('x-request-file', {});

  // ── status and download progress ────────────────────────────────────────────
  function status(text, cls) { const s = $('status'); if (s) { s.textContent = text; s.className = cls || 'dim'; } }
  const dl = { files: new Map(), done: false, firstMs: null, lastMs: null };
  m1.phases = m1.phases || { boot_called: null, first_progress: null, download_done: null, first_serial: null, prompt: null };
  // `total` can be absent on a chunked response: an unsized file is counted as
  // unsized, never scored as 0 bytes (a bar that under-reports runs backwards).
  function progress(e, label) {
    const prev = dl.files.get(e.file_name) || { loaded: 0, total: 0 };
    dl.files.set(e.file_name, { loaded: e.loaded || 0, total: (e.total && e.total > 0) ? e.total : prev.total });
    if (dl.firstMs === null) { dl.firstMs = since(); m1.phases.first_progress = dl.firstMs; }
    dl.lastMs = since();
    if (dl.done) return;
    let loaded = 0, total = 0, unsized = 0;
    for (const f of dl.files.values()) { loaded += f.loaded; total += f.total; if (!f.total) unsized++; }
    m1.dl = { loaded, total, files: dl.files.size, unsized };
    label = label || 'fetching the machine…';
    status(total ? `${label} ${mib(loaded)} / ${mib(total)} MiB` : `${label} ${mib(loaded)} MiB`);
  }
  const downloadDone = () => { if (!dl.done) { dl.done = true; m1.phases.download_done = since(); return true; } return false; };

  // ── snapshot identity ───────────────────────────────────────────────────────
  // An identity, not a security check: the bytes already came by content hash. Two
  // FNV-1a 32 passes, because crypto.subtle is absent outside a secure context (a
  // phone on http://<lan-ip>) and this must give the same answer there. The
  // snapshot builders record the ids THIS function computed, so they cannot disagree.
  function idHash(u8) {
    let a = 0x811c9dc5, b = 0x01000193 ^ 0x9e3779b9;
    for (let i = 0; i < u8.length; i++) {
      a = Math.imul(a ^ u8[i], 0x01000193) >>> 0;
      b = Math.imul(b ^ u8[i], 0x01000193) >>> 0;
    }
    return a.toString(16).padStart(8, '0') + b.toString(16).padStart(8, '0') + ':' + u8.length;
  }
  const SNAPSHOT_BUILD = !hosted && new URLSearchParams(location.search).get('snapshot') === 'build';
  m1.snapshot = { used: false, why: 'not checked' };
  // Every key in `ids` must match the snapshot's record, or the machine boots cold.
  function snapshotFits(meta, ids) {
    const st = m1.snapshot;
    st.ids = ids;
    if (SNAPSHOT_BUILD) { st.why = 'building one'; return false; }
    if (powerOn.boot === 'cold') { st.why = 'a boot from scratch was asked for'; return false; }
    if (!meta) { st.why = 'none published'; return false; }
    let rec;
    try { rec = JSON.parse(new TextDecoder().decode(meta)); } catch (e) { st.why = 'snapshot.json unreadable'; return false; }
    for (const k of Object.keys(ids)) {
      if ((rec[k] || null) !== (ids[k] || null)) { st.why = `built for a different ${k}`; return false; }
    }
    st.why = 'fits';
    return true;
  }

  // ── power ───────────────────────────────────────────────────────────────────
  // A restart is a RELOAD of the document, never an emulator rebuilt in place: the
  // page holds many pieces of per-machine state and a reload resets every one by
  // construction. How to start next crosses the reload in window.name, which a
  // sandboxed opaque-origin frame keeps (measured, Firefox and Chrome) and which is
  // the only storage such a frame has. `?boot=cold` asks for the same by URL.
  const powerOn = (() => {
    try {
      const v = JSON.parse(window.name || 'null');
      if (v && v.entityVm) { window.name = ''; return { boot: v.entityVm.boot === 'cold' ? 'cold' : 'resume', note: v.entityVm.note || null, by: v.entityVm.by || null }; }
    } catch (e) {}
    let byUrl = null;
    try { byUrl = new URLSearchParams(location.search).get('boot'); } catch (e) {}
    return byUrl === 'cold' ? { boot: 'cold', note: null, by: 'url' } : { boot: 'resume', note: null, by: null };
  })();
  m1.power = { started: powerOn, actions: [] };

  let powering = false;
  let powerHooks = { save: async () => 'nothing needed saving', off: () => {} };
  async function power(what, by) {
    if (powering) return;
    const log = { what, by: by || 'menu', at: since() };
    m1.power.actions.push(log);
    if (what === 'start' || what === 'start-cold') { startAgain(what === 'start-cold' ? 'cold' : 'resume', null, log.by); return; }
    powering = true;
    closeMenus();
    let note;
    try {
      // Bounded: a wedged guest must not make the power button the thing that hangs.
      note = await Promise.race([
        powerHooks.save(what),
        new Promise(res => setTimeout(() => res('the machine did not answer, so nothing more was saved'), 20000)),
      ]);
    } catch (e) { note = 'saving failed: ' + String(e).slice(0, 120); }
    log.save = note;
    if (what === 'off') {
      try { powerHooks.off(note); } catch (e) {}
      powering = false;
      status('off', 'bad');
      const b = $('power'); if (b) b.classList.add('off');
      for (const el of document.querySelectorAll('#powermenu [data-power]')) el.hidden = !el.dataset.power.startsWith('start');
      log.done = since();
      return;
    }
    startAgain(what === 'cold' ? 'cold' : 'resume', `restarted -- ${note}`, log.by);
  }
  function startAgain(boot, note, by) {
    status(boot === 'cold' ? 'restarting from scratch…' : 'restarting…');
    window.name = JSON.stringify({ entityVm: { boot, note, by } });
    location.reload();
  }

  // ── icons ───────────────────────────────────────────────────────────────────
  // Inline SVG, never a font glyph. A phone reported the power button as an empty
  // box (2026-09-14): U+23FB ⏻ is missing from common Android fonts, and ⓘ / ⌨ are
  // not reliable either. An SVG draws the same everywhere, in currentColor.
  const ICONS = {
    power: '<path d="M12 3v8"/><path d="M6.4 6.6a8 8 0 1 0 11.2 0"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6"/><circle cx="12" cy="7.5" r="0.6" fill="currentColor"/>',
    keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6 10h.01M9.5 10h.01M13 10h.01M16.5 10h.01M6 14h.01M18 14h.01M9 14h6"/>',
    sound: '<path d="M4 9h4l5-4v14l-5-4H4z"/><path d="M16.5 8.5a5 5 0 0 1 0 7"/><path d="M19 6a8.5 8.5 0 0 1 0 12"/>',
    muted: '<path d="M4 9h4l5-4v14l-5-4H4z"/><path d="M17 9l5 6M22 9l-5 6"/>',
    floppy: '<path d="M4 4h13l3 3v13H4z"/><path d="M8 4v5h8V4"/><rect x="7" y="13" width="10" height="7"/>',
  };
  function icon(name) {
    return `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" ` +
      `stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" style="display:block">${ICONS[name] || ''}</svg>`;
  }

  // ── chrome: the ⓘ and ⏻ buttons, their menus ────────────────────────────────
  const CSS = `
  #power, #info { flex:0 0 auto; font:14px/1 system-ui,sans-serif; padding:4px 7px; background:#333;
                  color:#ddd; border:1px solid #444; border-radius:3px; }
  #power.off { background:#7a2a24; border-color:#9a3a32; }
  #powermenu { position:fixed; top:34px; right:6px; z-index:10; background:#2a2a2a;
               border:1px solid #444; border-radius:4px; padding:4px; min-width:230px;
               max-width:calc(100vw - 12px); box-shadow:0 4px 14px rgba(0,0,0,.5); }
  #powermenu button { display:block; width:100%; text-align:left; font:13px system-ui,sans-serif;
                      padding:9px 10px; background:none; color:#ddd; border:0; border-radius:3px; }
  #powermenu button:hover, #powermenu button:focus { background:#3a3a3a; }
  #powermenu .note { padding:6px 10px 4px; font-size:11px; color:#888; white-space:normal; }
  #aboutpanel { position:fixed; inset:34px 6px auto 6px; z-index:11; max-height:calc(100vh - 44px);
                overflow:auto; background:#222; color:#ddd; border:1px solid #444; border-radius:4px;
                padding:10px 12px 14px; font:13px/1.45 system-ui,sans-serif; box-shadow:0 4px 18px rgba(0,0,0,.6); }
  #aboutpanel h2 { font-size:15px; margin:0 0 6px; } #aboutpanel h3 { font-size:13px; margin:14px 0 4px; }
  #aboutpanel p { margin:4px 0; color:#bbb; } #aboutpanel a { color:#79b8ff; }
  #aboutpanel table { border-collapse:collapse; width:100%; font-size:12px; }
  #aboutpanel td, #aboutpanel th { text-align:left; padding:3px 6px; border-top:1px solid #333; vertical-align:top; }
  #aboutpanel th { color:#999; font-weight:500; } #aboutpanel .wrap { overflow-x:auto; }
  #aboutpanel .close { float:right; background:#333; color:#ddd; border:1px solid #444; border-radius:3px; padding:3px 9px; }
  #aboutpanel .open { color:#e3b341; }
  #aboutpanel input { width:100%; box-sizing:border-box; margin:6px 0; padding:5px 7px; background:#1a1a1a; color:#ddd; border:1px solid #444; border-radius:3px; }
  `;
  const esc = s => String(s == null ? '' : s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const link = u => u ? `<a href="${esc(u)}" target="_blank" rel="noopener noreferrer">${esc(u.replace(/^https?:\/\//, ''))}</a>` : '';

  function closeMenus() {
    const pm = $('powermenu'); if (pm) pm.hidden = true;
    const pb = $('power'); if (pb) pb.setAttribute('aria-expanded', 'false');
    const ap = $('aboutpanel'); if (ap) ap.hidden = true;
  }

  // chrome({ note, about, hooks }) -- adds ⓘ and ⏻ to #bar. `about` is rendered by
  // renderAbout; `hooks.save(what)` returns the sentence the restarted machine shows,
  // `hooks.off(note)` shows the machine is off.
  function chrome(opts) {
    opts = opts || {};
    if (opts.hooks) powerHooks = Object.assign({}, powerHooks, opts.hooks);
    const style = document.createElement('style'); style.textContent = CSS; document.head.appendChild(style);
    const bar = $('bar');
    const info = document.createElement('button');
    info.id = 'info'; info.innerHTML = icon('info'); info.title = 'What this machine is made of, and where its source is';
    info.setAttribute('aria-label', 'About this machine');
    const pwr = document.createElement('button');
    pwr.id = 'power'; pwr.innerHTML = icon('power'); pwr.title = 'Restart or turn off the machine';
    pwr.setAttribute('aria-label', 'Power');
    pwr.setAttribute('aria-haspopup', 'true'); pwr.setAttribute('aria-expanded', 'false');
    bar.appendChild(info); bar.appendChild(pwr);
    const menu = document.createElement('div');
    menu.id = 'powermenu'; menu.hidden = true;
    menu.innerHTML = `
      <button data-power="restart">Restart</button>
      <button data-power="cold">Restart and show the boot</button>
      <button data-power="off">Turn off</button>
      <button data-power="start" hidden>Start</button>
      <button data-power="start-cold" hidden>Start and show the boot</button>
      <div class="note">${esc(opts.note || '')}</div>`;
    document.body.appendChild(menu);
    const panel = document.createElement('div');
    panel.id = 'aboutpanel'; panel.hidden = true;
    document.body.appendChild(panel);
    const place = el => { el.style.top = Math.round(bar.getBoundingClientRect().bottom + 2) + 'px'; };
    pwr.addEventListener('click', e => {
      e.stopPropagation();
      if (!menu.hidden) { closeMenus(); return; }
      closeMenus(); place(menu); menu.hidden = false; pwr.setAttribute('aria-expanded', 'true');
    });
    let about = Object.assign({}, opts.about || {});
    let more = opts.loadAbout || null;
    const bindFilters = () => {
      for (const filter of panel.querySelectorAll('input[data-filter]')) filter.addEventListener('input', () => {
        const q = filter.value.trim().toLowerCase();
        const body = filter.nextElementSibling && filter.nextElementSibling.querySelector('tbody[data-filterable]');
        if (body) for (const tr of body.rows) tr.hidden = !!q && !tr.textContent.toLowerCase().includes(q);
      });
    };
    const rerender = () => { panel.innerHTML = renderAbout(about); bindFilters(); };
    rerender();
    info.addEventListener('click', async e => {
      e.stopPropagation();
      const open = panel.hidden; closeMenus();
      if (!open) return;
      place(panel); panel.hidden = false;
      // The long part (a package list) is fetched the first time someone looks.
      if (more) {
        const load = more; more = null;
        about.loading = true; rerender();
        try { const extra = await load(); about = Object.assign({}, about, extra || {}); }
        catch (err) { about.obligations = (about.obligations || []).concat([{ text: 'the package list could not be loaded: ' + String(err).slice(0, 120), open: true }]); }
        about.loading = false; rerender();
      }
    });
    panel.addEventListener('click', e => {
      e.stopPropagation();
      if (e.target.classList.contains('close')) closeMenus();
    });
    for (const b of menu.querySelectorAll('[data-power]')) b.addEventListener('click', e => { e.stopPropagation(); power(b.dataset.power, 'menu'); });
    document.addEventListener('click', e => { if (!menu.contains(e.target) && !panel.contains(e.target)) closeMenus(); });
    addEventListener('keydown', e => { if (e.key === 'Escape') closeMenus(); });
  }

  // about = { title, summary, components: [{name, what, version, licence, home, source, notes}],
  //           tables?: [{caption, note, rows: [[name, version, licence, origin, sourceUrl]]}],
  //           obligations?: [{text, open}] }
  // Information, not a compliance claim: it says what each piece is and exactly
  // where its source is, and says plainly which obligations are still open.
  function renderAbout(a) {
    let h = `<button class="close" aria-label="Close">✕</button><h2>${esc(a.title || 'About this machine')}</h2>`;
    if (a.summary) h += `<p>${esc(a.summary)}</p>`;
    if (a.components && a.components.length) {
      h += `<h3>What it is made of</h3><div class="wrap"><table><thead><tr><th>Component</th><th>Version</th><th>Licence</th><th>Source</th></tr></thead><tbody>`;
      for (const c of a.components) {
        h += `<tr><td><b>${esc(c.name)}</b>${c.what ? `<br>${esc(c.what)}` : ''}${c.home ? `<br>${link(c.home)}` : ''}</td>` +
             `<td>${esc(c.version)}</td><td>${esc(c.licence)}</td>` +
             `<td>${c.source ? link(c.source) : ''}${c.notes ? `<br>${esc(c.notes)}` : ''}</td></tr>`;
      }
      h += `</tbody></table></div>`;
    }
    for (const t of a.tables || []) {
      if (!t.rows || !t.rows.length) continue;
      h += `<h3>${esc(t.caption)} (${t.rows.length})</h3>`;
      if (t.note) h += `<p>${esc(t.note)}</p>`;
      h += `<input data-filter placeholder="filter by name or licence"><div class="wrap"><table><thead><tr><th>Package</th><th>Version</th><th>Licence</th><th>Source</th></tr></thead><tbody data-filterable>`;
      for (const [name, version, licence, origin, url] of t.rows) {
        h += `<tr><td>${esc(name)}</td><td>${esc(version)}</td><td>${esc(licence)}</td><td>${url ? `<a href="${esc(url)}" target="_blank" rel="noopener noreferrer">${esc(origin || name)}</a>` : esc(origin)}</td></tr>`;
      }
      h += `</tbody></table></div>`;
    }
    if (a.loading) h += `<p class="dim">loading the package list…</p>`;
    if (a.obligations && a.obligations.length) {
      h += `<h3>Source obligations</h3>`;
      for (const o of a.obligations) h += `<p class="${o.open ? 'open' : ''}">${o.open ? '◌ ' : '● '}${esc(o.text)}</p>`;
    }
    return h;
  }

  // ── the clock shim (DESIGN-2026-09-14 §1.1) ─────────────────────────────────
  // v86 toggles the PIT refresh bit (port 0x61) every 15 µs of performance.now().
  // An origin that is not cross-origin isolated has that clock rounded to 1 ms, so a
  // guest delay loop polling the bit waits ~66x too long: KolibriOS 33 s -> 8.6 s.
  // Strictly increasing, never more than 1 ms ahead of the real clock. Must run
  // BEFORE libv86 reads the clock, i.e. before new V86().
  function clockShim() {
    if (performance.now.__entityShim) return false;
    if (typeof crossOriginIsolated !== 'undefined' && crossOriginIsolated) return false;   // already fine-grained
    const orig = performance.now.bind(performance);
    let last = 0;
    const shim = function () { const r = orig(); last = Math.min(Math.max(r, last + 0.015), r + 0.999); return last; };
    shim.__entityShim = true;
    performance.now = shim;
    m1.clockShim = true;
    return true;
  }

  // ── x-stats: what this machine tells the host's System Monitor ─────────────
  // The browser measures nothing about a sandboxed frame, so the emulator speaks
  // for itself (DESIGN-2026-09-14-c §4, a local extension like x-files): busy ms
  // over the span since the last report, instructions executed, and the wasm
  // memory the engine holds (guest RAM + video memory + JIT). Busy time is v86's
  // own main loop, timed by wrapping do_tick on the INSTANCE -- yield_callback
  // calls this.do_tick(), so the instance property is the one that runs. In
  // Firefox this loop shares the host's thread, so it is exactly the work that
  // freezes the tab; in Chrome it is this frame's own process.
  // `getEmulator` is read every report, so a restart's new instance is picked up.
  function reportStats(getEmulator) {
    if (!hosted || m1.stats) return;
    m1.stats = { reports: 0, wrapped: 0, last: null };
    const wrapped = new WeakSet();
    let busy = 0, lastAt = performance.now(), lastCount = null, lastCpu = null;
    setInterval(() => {
      const emu = getEmulator && getEmulator();
      const v86 = emu && emu.v86;
      if (v86 && !wrapped.has(v86) && typeof v86.do_tick === 'function') {
        const tick = v86.do_tick;
        v86.do_tick = function () {
          const t = performance.now();
          try { return tick.apply(this, arguments); } finally { busy += performance.now() - t; }
        };
        wrapped.add(v86);
        m1.stats.wrapped++;
      }
      const now = performance.now();
      const span = now - lastAt;
      const report = { busy_ms: Math.min(busy, span), span_ms: span };
      const cpu = v86 && v86.cpu;
      if (emu && typeof emu.get_instruction_counter === 'function') {
        const count = emu.get_instruction_counter();
        // The counter is a u32 that wraps; a different cpu (a restart) starts over.
        if (lastCount !== null && cpu === lastCpu) report.instructions = (count - lastCount + 0x100000000) % 0x100000000;
        lastCount = count; lastCpu = cpu;
      }
      const mem = cpu && cpu.wasm_memory && cpu.wasm_memory.buffer;
      if (mem) report.memory_bytes = mem.byteLength;
      busy = 0; lastAt = now;
      if (!emu) return;            // not built yet: nothing to say, and silence reads as "not reporting"
      send('x-stats', report);
      m1.stats.reports++; m1.stats.last = report;
    }, 1000);
  }

  // x-view: what this page's screen is and how much of the frame is not screen
  // (the bar), so the host can make its WINDOW as tall as the screen needs at the
  // frame's width -- a 4:3 desktop in a short, wide window is otherwise drawn
  // small with bars either side. Returns a function the page calls whenever it
  // re-fits; it posts only on a change, because the host's answer resizes this
  // frame and the page re-fits again on that.
  function reportView(stage) {
    let last = '';
    return (w, h) => {
      if (!hosted || !(w > 0) || !(h > 0)) return;
      const r = { screen_w: w, screen_h: h,
                  extra_w: Math.max(0, innerWidth - stage.clientWidth),
                  extra_h: Math.max(0, innerHeight - stage.clientHeight) };
      const k = JSON.stringify(r);
      if (k === last) return;
      last = k; m1.view = r;
      send('x-view', r);
    };
  }

  // ── a POSIX ustar archive ───────────────────────────────────────────────────
  // Names up to 255 bytes (prefix + name); longer ones are skipped and counted
  // through onSkip, never truncated into a different path.
  function makeTar(entries, onSkip) {
    const enc = new TextEncoder();
    const blocks = [];
    const field = (buf, off, len, str) => { const b = enc.encode(str); buf.set(b.subarray(0, len), off); };
    const octal = (n, len) => n.toString(8).padStart(len - 1, '0') + '\0';
    for (const { f, data } of entries) {
      const name = enc.encode(f.path);
      let pre = '', base = f.path;
      if (name.length > 100) {
        const cut = f.path.lastIndexOf('/', f.path.length - 1);
        pre = cut > 0 ? f.path.slice(0, cut) : '';
        base = cut > 0 ? f.path.slice(cut + 1) : f.path;
        if (enc.encode(pre).length > 155 || enc.encode(base).length > 100) { if (onSkip) onSkip(f.path); continue; }
      }
      const h = new Uint8Array(512);
      field(h, 0, 100, base);
      field(h, 100, 8, octal(f.mode & 0o7777, 8));
      field(h, 108, 8, octal(0, 8));
      field(h, 116, 8, octal(0, 8));
      field(h, 124, 12, octal(data.length, 12));
      field(h, 136, 12, octal(Math.max(0, Math.floor(f.mtime)), 12));
      field(h, 148, 8, '        ');
      h[156] = 0x30;
      field(h, 257, 6, 'ustar\0');
      field(h, 263, 2, '00');
      field(h, 345, 155, pre);
      let sum = 0; for (const b of h) sum += b;
      field(h, 148, 8, sum.toString(8).padStart(6, '0') + '\0 ');
      blocks.push(h, data);
      const pad = (512 - (data.length % 512)) % 512;
      if (pad) blocks.push(new Uint8Array(pad));
    }
    blocks.push(new Uint8Array(1024));
    const out = new Uint8Array(blocks.reduce((a, b) => a + b.length, 0));
    let o = 0; for (const b of blocks) { out.set(b, o); o += b.length; }
    return out;
  }

  const api = window.EntityVm = {
    HOST, APP, hosted, $, t0, since, token, mib, kb, strip, exactBuffer,
    send, on, start, asset, fetchBytes, work, download, sendFile, requestFile,
    status, progress, downloadDone, idHash, snapshotFits, SNAPSHOT_BUILD,
    powerOn, power, startAgain, isPowering: () => powering, chrome, closeMenus, renderAbout,
    clockShim, makeTar, icon, reportStats, reportView, onUnmatchedFileResult: null,
  };
})();

(async function () {
  const P = self.__P;
  const R = { startedAt: new Date().toISOString(), log: [] };
  const prog = (s) => { window.__PROGRESS__ = s; R.log.push([Math.round(performance.now()), s]); };
  const SANDBOX = { srcdoc: 'allow-scripts allow-downloads', urlSame: 'allow-scripts allow-same-origin', urlOpaque: 'allow-scripts allow-downloads' };
  R.sandbox = SANDBOX;
  try {
    // ---- observers in the TOP page, from the very start
    const topEntries = [];
    R.topObserve = {};
    for (const type of ['longtask', 'long-animation-frame']) {
      try { new PerformanceObserver((l) => { for (const e of l.getEntries()) topEntries.push(P.serEntry(e)); }).observe({ type, buffered: true }); R.topObserve[type] = 'observe() returned'; }
      catch (e) { R.topObserve[type] = 'THROW ' + P.errStr(e); }
    }
    // ---- rAF + setTimeout-lag recorders
    const raf = []; let rafOn = true;
    const rafLoop = (ts) => { raf.push(performance.now()); if (rafOn) requestAnimationFrame(rafLoop); };
    requestAnimationFrame(rafLoop);
    const tick = []; let tickOn = true;
    const tickLoop = () => { tick.push(performance.now()); if (tickOn) setTimeout(tickLoop, 10); };
    setTimeout(tickLoop, 10);

    // ---- contexts
    let nextId = 1; const pending = new Map();
    const targets = {};
    const onReply = (d) => { if (d && d.id && pending.has(d.id)) { const f = pending.get(d.id); pending.delete(d.id); f(d); } };
    const hellos = new Map();
    window.addEventListener('message', (ev) => {
      if (ev.data && ev.data.hello) { for (const [k, f] of Object.entries(targets)) if (f.win && ev.source === f.win()) hellos.set(k, performance.now()); return; }
      onReply(ev.data);
    });
    const ask = (name, msg, timeout) => {
      const id = nextId++; msg.id = id;
      return P.withTimeout(() => new Promise((res) => { pending.set(id, res); targets[name].post(msg); }), timeout);
    };

    prog('worker');
    const worker = new Worker('worker.js');
    worker.onmessage = (ev) => onReply(ev.data);
    worker.onerror = (e) => { R.workerError = String(e.message); };
    targets.worker = { post: (m) => worker.postMessage(m) };

    prog('frames');
    const collectSrc = await (await fetch('collect.js')).text();
    const frameSrc = await (await fetch('frame.js')).text();
    for (const kind of ['srcdoc', 'urlSame', 'urlOpaque']) {
      const f = document.createElement('iframe');
      f.id = 'f-' + kind; f.name = 'n-' + kind;
      f.setAttribute('sandbox', SANDBOX[kind]);
      f.setAttribute('allow', 'screen-wake-lock *');
      f.width = 200; f.height = 60;
      if (kind === 'srcdoc') f.setAttribute('srcdoc', '<!doctype html><body>srcdoc frame<script>' + collectSrc + '<\/script><script>' + frameSrc + '<\/script>');
      else f.src = 'frame.html?kind=' + kind;
      document.body.appendChild(f);
      targets[kind] = { win: () => f.contentWindow, post: (m) => f.contentWindow.postMessage(m, '*') };
    }
    const tW = performance.now();
    while (hellos.size < 3 && performance.now() - tW < 10000) await P.sleep(50);
    R.frameHello = Object.fromEntries(['srcdoc', 'urlSame', 'urlOpaque'].map((k) => [k, hellos.has(k)]));
    const wHello = await ask('worker', { cmd: 'hello' }, 5000);
    R.workerHello = wHello.ok;
    const ctxs = ['worker', ...['srcdoc', 'urlSame', 'urlOpaque'].filter((k) => hellos.has(k))];

    // ---- 1. environment per context
    prog('collect');
    R.env = { top: await P.collect() };
    for (const c of ctxs) { const r = await ask(c, { cmd: 'collect' }, 20000); R.env[c] = r.ok ? r.v.data : r.err; }

    // ---- 3/4. busy windows
    prog('busy');
    await P.sleep(800);
    const windows = [];
    const idleStart = performance.now(); await P.sleep(1000);
    windows.push({ who: 'idle-baseline', sent: idleStart, start: idleStart, end: idleStart + 300 });
    await P.sleep(400);
    // (a) top
    { const s = performance.now(); await new Promise((r) => setTimeout(r, 0)); const bs = performance.now(); P.busy(300); const be = performance.now(); windows.push({ who: 'top', sent: s, start: bs, end: be }); }
    await P.sleep(1500);
    for (const c of ctxs) {
      const sent = performance.now();
      const replyP = ask(c, { cmd: 'busy', ms: 300 }, 5000);
      await P.sleep(60);
      const pingSent = performance.now();
      const pingP = ask(c, { cmd: 'ping' }, 5000).then((r) => ({ ok: r.ok, rttMs: Math.round(performance.now() - pingSent) }));
      const rep = await replyP; const ping = await pingP;
      const w = { who: c, sent, ping };
      if (rep.ok) { w.start = rep.v.timeOrigin + rep.v.start - performance.timeOrigin; w.end = rep.v.timeOrigin + rep.v.end - performance.timeOrigin; w.replyReceived = performance.now(); w.ctxTimeOriginDeltaMs = rep.v.timeOrigin - performance.timeOrigin; }
      else w.err = rep.err;
      windows.push(w);
      await P.sleep(1500);
    }
    await P.sleep(500);
    rafOn = false; tickOn = false;
    const gapStats = (arr, a, b) => {
      let maxGap = 0, n = 0; const big = [];
      for (let i = 1; i < arr.length; i++) { if (arr[i] < a || arr[i - 1] > b) continue; const g = arr[i] - arr[i - 1]; n++; if (g > maxGap) maxGap = g; if (g > 50) big.push(Math.round(g)); }
      return { samples: n, maxGapMs: Math.round(maxGap * 10) / 10, gapsOver50ms: big };
    };
    // overall rAF cadence
    const allGaps = raf.slice(1).map((v, i) => v - raf[i]).sort((x, y) => x - y);
    R.rafOverall = { frames: raf.length, medianGapMs: allGaps.length ? Math.round(allGaps[allGaps.length >> 1] * 100) / 100 : null };
    const tickGaps = tick.slice(1).map((v, i) => v - tick[i]).sort((x, y) => x - y);
    R.tickOverall = { ticks: tick.length, medianGapMs: tickGaps.length ? Math.round(tickGaps[tickGaps.length >> 1] * 100) / 100 : null };
    const frameEntries = {};
    for (const c of ['srcdoc', 'urlSame', 'urlOpaque'].filter((k) => hellos.has(k))) { const r = await ask(c, { cmd: 'entries' }, 5000); frameEntries[c] = r.ok ? r.v.data : r.err; }
    R.busy = windows.map((w) => {
      const s = w.start ?? w.sent, e = (w.end ?? w.sent + 300);
      const lo = Math.min(w.sent, s) - 20, hi = e + 150;
      return Object.assign({}, w, {
        sent: Math.round(w.sent), start: w.start && Math.round(w.start), end: w.end && Math.round(w.end), replyReceived: w.replyReceived && Math.round(w.replyReceived),
        topRaf: gapStats(raf, lo, hi), topTimeout10ms: gapStats(tick, lo, hi),
        topEntriesInWindow: topEntries.filter((x) => x.startTime >= lo && x.startTime <= hi),
      });
    });
    R.topEntriesAll = topEntries;
    R.frameOwnEntries = frameEntries;

    // ---- 5. resource timing
    prog('fetch');
    R.fetch = { top: await P.doFetch('top') };
    for (const c of ctxs) { const r = await ask(c, { cmd: 'fetch', from: c }, 15000); R.fetch[c] = r.ok ? r.v.data : r.err; }
    await P.sleep(500);
    R.topResourceEntries = performance.getEntriesByType('resource').map(P.serRes).filter((e) => /blob|frame|worker|collect/.test(e.name));

    // ---- workers spawned from inside iframes
    prog('frame-workers');
    R.frameWorkers = {};
    for (const c of ['srcdoc', 'urlSame', 'urlOpaque'].filter((k) => hellos.has(k))) { const r = await ask(c, { cmd: 'workers' }, 15000); R.frameWorkers[c] = r.ok ? r.v.data : r.err; }

    // ---- pressure, each context sequentially
    prog('pressure');
    R.pressure = { top: await P.pressure(5000) };
    for (const c of ctxs) { const r = await ask(c, { cmd: 'pressure', ms: 5000 }, 15000); R.pressure[c] = r.ok ? r.v.data : r.err; }

    // ---- pressure under whole-machine load (only with ?stress=1): does the state ever leave nominal?
    if (location.search.includes('stress=1')) {
      prog('pressure-stress');
      R.pressureStress = await (async () => {
        if (typeof PressureObserver !== 'function') return { typeof: typeof PressureObserver };
        const n = navigator.hardwareConcurrency || 4;
        const url = URL.createObjectURL(new Blob(['onmessage=e=>{const end=performance.now()+e.data;let x=0;while(performance.now()<end){x++} postMessage(x)}'], { type: 'text/javascript' }));
        const recs = [];
        const obs = new PressureObserver((rs) => { for (const x of rs) recs.push({ state: x.state, time: Math.round(x.time) }); });
        const o = await P.withTimeout(() => obs.observe('cpu', { sampleInterval: 500 }), 3000);
        if (!o.ok) return { observe: o.err };
        await P.sleep(2000);
        const t0 = performance.now(); const ws = [];
        for (let i = 0; i < n; i++) { const w = new Worker(url); w.postMessage(8000); ws.push(w); }
        await P.sleep(9000); ws.forEach((w) => w.terminate());
        const t1 = performance.now();
        await P.sleep(4000); obs.disconnect();
        return { workers: n, loadStart: Math.round(t0), loadEnd: Math.round(t1), records: recs };
      })();
    }

    // ---- memory measurement (top gets the long budget)
    prog('memory-top');
    R.memory = { top: await P.memory(60000) };
    for (const c of ctxs) { prog('memory-' + c); const r = await ask(c, { cmd: 'memory', budget: 30000 }, 35000); R.memory[c] = r.ok ? r.v.data : r.err; }
  } catch (e) {
    R.fatal = P.errStr(e) + ' ' + (e && e.stack);
  }
  prog('done');
  window.__RESULT__ = R;
})();

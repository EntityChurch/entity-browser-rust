// Shared collector: runs in Window (top + iframes) and DedicatedWorker.
(function () {
  const errStr = (e) => (e && e.name ? e.name : 'Error') + ': ' + (e && e.message !== undefined ? e.message : String(e));
  const t = (f) => { try { return f(); } catch (e) { return 'THROW ' + errStr(e); } };
  const withTimeout = (mk, ms) => {
    let p;
    try { p = Promise.resolve(mk()); } catch (e) { return Promise.resolve({ ok: false, err: 'THROW ' + errStr(e) }); }
    return Promise.race([
      p.then((v) => ({ ok: true, v }), (e) => ({ ok: false, err: errStr(e) })),
      new Promise((r) => setTimeout(() => r({ ok: false, err: 'TIMEOUT ' + ms + 'ms' }), ms)),
    ]);
  };
  const busy = (ms) => { const e = performance.now() + ms; let n = 0; while (performance.now() < e) n++; return n; };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  function timerRes(n) {
    let last = performance.now(), min = Infinity; const seen = new Set();
    const t0 = performance.now();
    for (let i = 0; i < n; i++) {
      const v = performance.now(); const d = v - last;
      if (d > 0) { if (d < min) min = d; if (seen.size < 2000) seen.add(Math.round(d * 1e6) / 1e6); }
      last = v;
    }
    return { calls: n, wallMs: performance.now() - t0, minNonZeroMs: min === Infinity ? null : min,
      smallestDistinctMs: [...seen].sort((a, b) => a - b).slice(0, 6) };
  }

  async function collect() {
    const r = {};
    r.global = t(() => (typeof window !== 'undefined' && self === window) ? 'Window' : self.constructor && self.constructor.name);
    r.origin = t(() => self.origin);
    r.href = t(() => String(self.location.href).slice(0, 100));
    r.isSecureContext = t(() => self.isSecureContext);
    r.crossOriginIsolated = t(() => self.crossOriginIsolated);
    r.SharedArrayBuffer = typeof SharedArrayBuffer;
    r.sabConstruct = t(() => new SharedArrayBuffer(8).byteLength);
    r.performance_memory = t(() => performance.memory ? { usedJSHeapSize: performance.memory.usedJSHeapSize, totalJSHeapSize: performance.memory.totalJSHeapSize, jsHeapSizeLimit: performance.memory.jsHeapSizeLimit } : String(performance.memory));
    r.measureUserAgentSpecificMemory = typeof performance.measureUserAgentSpecificMemory;
    r.deviceMemory = t(() => navigator.deviceMemory === undefined ? 'undefined' : navigator.deviceMemory);
    r.hardwareConcurrency = t(() => navigator.hardwareConcurrency);
    r.storage = typeof navigator.storage;
    if (navigator.storage) {
      const est = await withTimeout(() => navigator.storage.estimate(), 5000);
      r.storage_estimate = est.ok ? { usage: est.v.usage, quota: est.v.quota, usageDetails: est.v.usageDetails === undefined ? 'undefined' : est.v.usageDetails } : est.err;
      r.storage_persisted = typeof navigator.storage.persisted === 'function' ? (await withTimeout(() => navigator.storage.persisted(), 3000)) : typeof navigator.storage.persisted;
    }
    r.connection = t(() => navigator.connection ? { effectiveType: navigator.connection.effectiveType, downlink: navigator.connection.downlink, rtt: navigator.connection.rtt, saveData: navigator.connection.saveData, type: navigator.connection.type } : 'undefined');
    r.getBattery = typeof navigator.getBattery;
    if (typeof navigator.getBattery === 'function') {
      const b = await withTimeout(() => navigator.getBattery(), 3000);
      r.battery = b.ok ? { level: b.v.level, charging: b.v.charging, chargingTime: b.v.chargingTime, dischargingTime: b.v.dischargingTime } : b.err;
    }
    r.PressureObserver = typeof PressureObserver;
    if (typeof PressureObserver === 'function') r.pressure_knownSources = t(() => PressureObserver.knownSources);
    r.userAgentData = typeof navigator.userAgentData;
    if (navigator.userAgentData) {
      r.uaData_low = t(() => ({ brands: navigator.userAgentData.brands, mobile: navigator.userAgentData.mobile, platform: navigator.userAgentData.platform }));
      const h = await withTimeout(() => navigator.userAgentData.getHighEntropyValues(['platform', 'architecture', 'model', 'platformVersion', 'bitness']), 3000);
      r.uaData_high = h.ok ? h.v : h.err;
    }
    r.userAgent = t(() => navigator.userAgent);
    r.timeOrigin = performance.timeOrigin;
    r.timer_10k = timerRes(10000);
    r.timer_1M = timerRes(1000000);
    r.scheduler = typeof self.scheduler;
    r.scheduler_yield = t(() => self.scheduler ? typeof self.scheduler.yield : 'n/a');
    r.scheduler_postTask = t(() => self.scheduler ? typeof self.scheduler.postTask : 'n/a');
    r.navigator_scheduling = typeof navigator.scheduling;
    r.isInputPending = t(() => navigator.scheduling ? typeof navigator.scheduling.isInputPending : 'n/a');
    if (navigator.scheduling && typeof navigator.scheduling.isInputPending === 'function') r.isInputPending_value = t(() => navigator.scheduling.isInputPending());
    r.wasm_memory = t(() => {
      const m = new WebAssembly.Memory({ initial: 1, maximum: 1000 });
      const before = m.buffer.byteLength; const ret = m.grow(15); const after = m.buffer.byteLength;
      return { before, growReturned: ret, after, toResizableBuffer: typeof m.toResizableBuffer };
    });
    r.wasm_shared_memory = t(() => { const m = new WebAssembly.Memory({ initial: 1, maximum: 10, shared: true }); return { byteLength: m.buffer.byteLength, bufferType: Object.prototype.toString.call(m.buffer) }; });
    r.supportedEntryTypes = t(() => PerformanceObserver.supportedEntryTypes);
    r.eventCounts = typeof performance.eventCounts;
    r.navigator_gpu = typeof navigator.gpu;
    r.wakeLock = typeof navigator.wakeLock;
    if (typeof document !== 'undefined') r.visibilityState = document.visibilityState;
    return r;
  }

  async function pressure(busyMs) {
    const out = { typeof: typeof PressureObserver };
    if (typeof PressureObserver !== 'function') return out;
    out.knownSources = t(() => PressureObserver.knownSources);
    const recs = []; let obs;
    try { obs = new PressureObserver((rs) => { for (const x of rs) recs.push({ source: x.source, state: x.state, time: x.time, ownContributionEstimate: x.ownContributionEstimate }); }); }
    catch (e) { out.ctor = 'THROW ' + errStr(e); return out; }
    const o = await withTimeout(() => obs.observe('cpu', { sampleInterval: 500 }), 3000);
    out.observe = o.ok ? 'resolved' : o.err;
    if (!o.ok) { t(() => obs.disconnect()); return out; }
    const s = performance.now();
    while (performance.now() - s < busyMs) { busy(80); await sleep(0); }
    await sleep(1000);
    out.records = recs.concat(t(() => obs.takeRecords().map((x) => ({ source: x.source, state: x.state, time: x.time }))) || []);
    t(() => obs.disconnect());
    return out;
  }

  async function memory(budget) {
    if (typeof performance.measureUserAgentSpecificMemory !== 'function') return { typeof: typeof performance.measureUserAgentSpecificMemory };
    const s = performance.now();
    const m = await withTimeout(() => performance.measureUserAgentSpecificMemory(), budget);
    const out = { elapsedMs: Math.round(performance.now() - s) };
    if (!m.ok) { out.err = m.err; return out; }
    out.bytes = m.v.bytes;
    out.breakdown = (m.v.breakdown || []).map((b) => ({ bytes: b.bytes, types: b.types, attribution: (b.attribution || []).map((a) => ({ url: a.url, scope: a.scope, container: a.container ? { id: a.container.id, src: a.container.src } : undefined })) }));
    return out;
  }

  async function doFetch(from) {
    const out = {};
    for (const f of ['data/blob.bin', 'data/blob-tao.bin']) {
      const r = await withTimeout(() => fetch(f + '?from=' + from).then((x) => x.arrayBuffer().then((b) => ({ status: x.status, bytes: b.byteLength }))), 5000);
      out[f] = r.ok ? r.v : r.err;
    }
    await sleep(300);
    out.ownEntries = t(() => performance.getEntriesByType('resource').filter((e) => e.name.includes('blob')).map(serRes));
    return out;
  }
  function serRes(e) {
    return { name: e.name.replace(/^https?:\/\/[^/]+/, ''), initiatorType: e.initiatorType, transferSize: e.transferSize, encodedBodySize: e.encodedBodySize, decodedBodySize: e.decodedBodySize, duration: Math.round(e.duration * 100) / 100, responseStatus: e.responseStatus, nextHopProtocol: e.nextHopProtocol };
  }
  function serEntry(e) {
    const o = { entryType: e.entryType, name: e.name, startTime: Math.round(e.startTime * 10) / 10, duration: Math.round(e.duration * 10) / 10 };
    if (e.entryType === 'longtask') o.attribution = (e.attribution || []).map((a) => ({ name: a.name, entryType: a.entryType, containerType: a.containerType, containerSrc: a.containerSrc, containerId: a.containerId, containerName: a.containerName }));
    if (e.entryType === 'long-animation-frame') {
      o.blockingDuration = e.blockingDuration; o.renderStart = e.renderStart; o.styleAndLayoutStart = e.styleAndLayoutStart;
      o.scripts = (e.scripts || []).map((s) => ({ invoker: s.invoker, invokerType: s.invokerType, sourceURL: s.sourceURL, sourceFunctionName: s.sourceFunctionName, sourceCharPosition: s.sourceCharPosition, windowAttribution: s.windowAttribution, duration: Math.round(s.duration * 10) / 10, startTime: Math.round(s.startTime * 10) / 10 }));
    }
    return o;
  }

  self.__P = { collect, pressure, memory, doFetch, busy, sleep, withTimeout, errStr, serEntry, serRes, t };
})();

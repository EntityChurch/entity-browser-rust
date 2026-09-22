// Runs inside each sandboxed iframe (srcdoc inlines it; URL frames load it).
(function () {
  const P = self.__P;
  const own = [];
  for (const type of ['longtask', 'long-animation-frame']) {
    try { new PerformanceObserver((l) => { for (const e of l.getEntries()) own.push(P.serEntry(e)); }).observe({ type, buffered: true }); } catch (e) { }
  }
  async function tryWorker(url) {
    let w;
    try { w = new Worker(url); } catch (e) { return 'THROW ' + P.errStr(e); }
    const r = await P.withTimeout(() => new Promise((res, rej) => { w.onmessage = (m) => res(m.data); w.onerror = (e) => rej(new Error('worker error event: ' + (e.message || ''))); w.postMessage({ id: 0, cmd: 'hello' }); }), 4000);
    w.terminate();
    return r.ok ? { ok: true, reply: typeof r.v } : r.err;
  }
  self.addEventListener('message', async (ev) => {
    const m = ev.data; if (!m || !m.cmd) return;
    const reply = (o) => parent.postMessage(Object.assign({ id: m.id }, o), '*');
    switch (m.cmd) {
      case 'collect': reply({ data: await P.collect() }); break;
      case 'busy': setTimeout(() => { const s = performance.now(); P.busy(m.ms); const e = performance.now(); reply({ start: s, end: e, timeOrigin: performance.timeOrigin }); }, 0); break;
      case 'ping': reply({ t: performance.now() }); break;
      case 'fetch': reply({ data: await P.doFetch(m.from) }); break;
      case 'entries': reply({ data: own }); break;
      case 'pressure': reply({ data: await P.pressure(m.ms) }); break;
      case 'memory': reply({ data: await P.memory(m.budget) }); break;
      case 'workers': reply({ data: {
        relativeUrl: await tryWorker('worker.js'),
        blobUrl: await P.t(() => tryWorker(URL.createObjectURL(new Blob(["onmessage=(e)=>postMessage({id:e.data.id,ok:1})"], { type: 'text/javascript' })))),
      } }); break;
    }
  });
  parent.postMessage({ hello: true }, '*');
})();

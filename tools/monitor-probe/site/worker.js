importScripts('collect.js');
const P = self.__P;
onmessage = async (ev) => {
  const m = ev.data; const reply = (o) => postMessage(Object.assign({ id: m.id }, o));
  switch (m.cmd) {
    case 'hello': reply({ ok: 1 }); break;
    case 'collect': reply({ data: await P.collect() }); break;
    case 'busy': setTimeout(() => { const s = performance.now(); P.busy(m.ms); const e = performance.now(); reply({ start: s, end: e, timeOrigin: performance.timeOrigin }); }, 0); break;
    case 'ping': reply({ t: performance.now() }); break;
    case 'fetch': reply({ data: await P.doFetch(m.from) }); break;
    case 'pressure': reply({ data: await P.pressure(m.ms) }); break;
    case 'memory': reply({ data: await P.memory(m.budget) }); break;
    case 'entries': reply({ data: [] }); break;
  }
};

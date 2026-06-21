// Worker bootstrap loader.
//
// Trunk's `data-type="worker"` pipeline emits the wasm-bindgen module
// (`entity-worker.js` + `entity-worker_bg.wasm`) but does not emit a
// loader that auto-instantiates the wasm inside the worker. This file
// is that loader.
//
// Also handles the **init-message race**: the main thread posts
// `Request::Init` immediately after spawning the worker, which arrives
// BEFORE wasm-bindgen finishes initializing and installs
// `self.onmessage`. Without this loader, that first Init message is
// silently dropped (delivered to whatever listeners exist at arrival
// time — none — and the wasm-side onmessage installed later never
// sees it). The proxy then hangs forever awaiting Ready.
//
// Solution: install an `addEventListener('message', ...)` BEFORE the
// wasm loads. Buffer everything that arrives before init completes.
// Once `wasm_bindgen()` resolves (which means `#[wasm_bindgen(start)]
// fn worker_main` ran and `install_onmessage` set `self.onmessage`),
// drain the buffer through that handler.

// `performance.now()` is the wall-clock time since navigation start
// (worker contexts have their own clock origin per spec, but the delta
// across `t0` is what we care about). Use to attribute the boot delay:
//   - `loader start → importScripts done` = fetch + parse of entity-worker.js
//   - `importScripts done → wasm init resolved` = wasm fetch + compile + worker_main
//   - rest of delay before Ready = Init handshake CBOR + handler registration
const t0 = performance.now();
console.log('[entity-worker] loader script start (t=0ms)');

let wasmReady = false;
const msgBuffer = [];

self.addEventListener('message', (evt) => {
    const d = evt.data;
    const desc =
        d instanceof Uint8Array ? `Uint8Array(${d.byteLength})`
        : d instanceof ArrayBuffer ? `ArrayBuffer(${d.byteLength})`
        : typeof d;
    if (!wasmReady) {
        msgBuffer.push(evt);
        console.log('[entity-worker] msg recv (buffered):', desc);
    } else {
        console.log('[entity-worker] msg recv:', desc);
    }
});

// postMessage diagnostic — wrap to log responses going back to main.
// Also dumps the first 32 bytes (hex) of each outbound payload so we can
// inspect the CBOR tag bytes when the proxy reports "failed to decode".
const origPostMessage = self.postMessage.bind(self);
self.postMessage = function (msg, ...rest) {
    let desc, hex = '';
    if (msg instanceof Uint8Array) {
        desc = `Uint8Array(${msg.byteLength})`;
        hex = Array.from(msg.slice(0, 32))
            .map((b) => b.toString(16).padStart(2, '0'))
            .join(' ');
    } else if (msg instanceof ArrayBuffer) {
        desc = `ArrayBuffer(${msg.byteLength})`;
    } else {
        desc = typeof msg;
    }
    console.log('[entity-worker] postMessage:', desc, hex ? `hex: ${hex}` : '');
    return origPostMessage(msg, ...rest);
};

try {
    importScripts('/entity-worker.js');
    const t = (performance.now() - t0).toFixed(1);
    console.log(`[entity-worker] importScripts done (t=${t}ms) — wasm_bindgen defined:`, typeof wasm_bindgen);
} catch (e) {
    console.error('[entity-worker] importScripts failed:', e);
    throw e;
}

console.log('[entity-worker] calling wasm_bindgen({ module_or_path: ... })');
wasm_bindgen({ module_or_path: '/entity-worker_bg.wasm' })
    .then(() => {
        const t = (performance.now() - t0).toFixed(1);
        console.log(
            `[entity-worker] wasm_bindgen init resolved (t=${t}ms) — worker_main has run.`,
            'self.onmessage present:', typeof self.onmessage,
            'buffered messages:', msgBuffer.length,
        );

        const handler = self.onmessage;
        wasmReady = true;
        if (!handler) {
            console.error('[entity-worker] worker_main did not install self.onmessage; buffer cannot be drained');
            return;
        }

        // Drain buffered messages into the wasm-side handler. These were
        // delivered to the worker before wasm-bindgen-host installed its
        // own listener; replay them now.
        for (const evt of msgBuffer) {
            try {
                handler(evt);
            } catch (e) {
                console.error('[entity-worker] handler threw on buffered message:', e);
            }
        }
        msgBuffer.length = 0;
    })
    .catch((err) => {
        console.error('[entity-worker] wasm init failed:', err);
    });

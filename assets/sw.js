// Entity Browser — Service Worker for app-shell offline cache.
//
// Purpose: when the user does a hard refresh while offline (or network
// is slow/unreachable), serve the static app shell + previously-fetched
// WASM/JS assets from cache so the page loads and the peer boots. The
// peer itself is offline-by-design (operates over OPFS-persistent local
// tree); the only thing network-required is the initial download of
// these static assets. This SW makes that download persist.
//
// Strategy — split by mutability of the URL, NOT one-size-fits-all:
//
//   * Content-HASHED assets (`-<hash>.js` / `-<hash>_bg.wasm`, emitted by
//     trunk) are IMMUTABLE: a given URL's bytes never change across
//     builds. Cache-first forever is correct and fast.
//
//   * Everything else is MUTABLE at a stable URL and MUST be network-first
//     (falling back to cache only when offline):
//       - `/` and `index.html` — the app-shell entry. It names the hashed
//         bundles for THIS build; serving a stale copy pins the old
//         bundle (and re-ships already-fixed panics — see below).
//       - `entity-worker.js` / `entity-worker_bg.wasm` / the loader — the
//         trunk worker bin is NOT content-hashed (fixed filenames, see
//         `project_trunk_worker_layout`). In Worker mode the worker IS the
//         peer; a cache-first stale worker is a stale runtime.
//       - `sw.js` itself.
//
// Why this design exists: a prior cache-first-everything SW served a stale
// app shell that pinned a pre-fix bundle. A Worker-arm content-site panic
// we had ALREADY fixed kept shipping, because the fix only reached the
// browser on the SECOND reload (stale-while-revalidate). Network-first on
// the mutable shell makes a deploy visible on the FIRST online load while
// preserving offline boot via the cache fallback.
//   (Supersedes the earlier "no version bumps / next-online-load is
//   fine" decision from the GAP-5 persistence investigation.)
//
//   - Same-origin GET only. Cross-origin + non-GET passes through.
//   - On offline-with-no-cache, return a plain 503 (rather than the
//     opaque browser-default failure). Lets the page show its existing
//     auto-retry banner.

// Bumped v1 → v2: the activate handler deletes every cache that isn't the
// current name, so this bump PURGES the stale v1 shell (old index.html +
// old hashed bundles) on activation — auto-recovering any client stuck on
// a pre-fix build. Bump again only if the cache contract itself changes.
const CACHE_NAME = 'entity-browser-shell-v2';

// The bare minimum that must always be available so the SW-driven boot
// path works at all. Everything else (hashed JS/WASM, worker loader)
// flows through the fetch handler and self-caches.
const CORE_ASSETS = ['/'];

// Content-hashed = immutable. Trunk emits `name-<16+ hex>.js` and
// `name-<16+ hex>_bg.wasm`; those URLs never change content, so they are
// safe to serve cache-first forever. NOT matched: `entity-worker.js`,
// `entity-worker_bg.wasm`, `entity-worker-loader.js`, `index.html`.
const HASHED_ASSET = /-[0-9a-f]{8,}(_bg)?\.(js|wasm)$/;

self.addEventListener('install', (event) => {
    event.waitUntil(
        caches.open(CACHE_NAME).then((cache) => cache.addAll(CORE_ASSETS))
    );
    // Activate immediately; don't wait for old tabs to close. Fine for
    // our app — no breaking-protocol concerns between SW versions.
    self.skipWaiting();
});

self.addEventListener('activate', (event) => {
    event.waitUntil(
        caches.keys().then((keys) =>
            Promise.all(
                keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))
            )
        )
    );
    // Take control of currently-open pages so the next fetch goes
    // through this SW (without requiring a reload).
    self.clients.claim();
});

self.addEventListener('fetch', (event) => {
    const req = event.request;
    // Only intercept same-origin GET. WebSocket / cross-origin / POSTs
    // pass through to the network unchanged.
    if (req.method !== 'GET') return;
    let url;
    try {
        url = new URL(req.url);
    } catch (_) {
        return;
    }
    if (url.origin !== self.location.origin) return;

    // Hashed, immutable asset → cache-first (fast, offline-tolerant, and
    // the URL guarantees freshness). Everything else (mutable shell +
    // non-hashed worker bundle) → network-first so a new deploy reaches
    // the browser on the first online load, falling back to cache offline.
    if (HASHED_ASSET.test(url.pathname)) {
        event.respondWith(cacheFirst(req));
    } else {
        event.respondWith(networkFirst(req));
    }
});

// Cache-first for immutable hashed assets, with a background revalidate as
// a belt-and-suspenders refresh (the URL changing per build is the real
// invalidation; this just heals a corrupted/partial cache entry).
async function cacheFirst(req) {
    const cache = await caches.open(CACHE_NAME);
    const cached = await cache.match(req);
    if (cached) return cached;

    const fresh = await fetch(req).then((resp) => {
        if (resp && resp.ok) cache.put(req, resp.clone()).catch(() => {});
        return resp;
    }).catch(() => null);
    if (fresh) return fresh;
    return offline503();
}

// Network-first for the mutable app shell + non-hashed worker bundle:
// always prefer the freshly-deployed bytes when online; fall back to the
// last-cached copy offline so the peer still boots; else a readable 503.
async function networkFirst(req) {
    const cache = await caches.open(CACHE_NAME);
    const fresh = await fetch(req).then((resp) => {
        if (resp && resp.ok) cache.put(req, resp.clone()).catch(() => {});
        return resp;
    }).catch(() => null);
    if (fresh) return fresh;

    // Offline: exact-URL match first.
    let cached = await cache.match(req);
    // For a NAVIGATION, the query string is just runtime routing
    // (`?systemrecovery=1`, `?worker=0`, `?site=…`) — it does not change WHICH
    // document to serve. `cache.match` is query-EXACT by default, so a
    // `/?anything` reload while offline used to 503 even though the shell is
    // cached at `/`. Fall back to the cached shell ignoring the query so an
    // offline reload of any app URL still boots from cache — including the
    // `?systemrecovery=1` BIOS screen, which must be reachable when offline.
    if (!cached && req.mode === 'navigate') {
        cached = await cache.match('/', { ignoreSearch: true });
    }
    if (cached) return cached;
    return offline503();
}

function offline503() {
    return new Response(
        'Entity Browser is offline and this asset is not cached. ' +
        'Reconnect to download the latest version.',
        {
            status: 503,
            statusText: 'Offline (no cached copy)',
            headers: { 'Content-Type': 'text/plain' },
        }
    );
}

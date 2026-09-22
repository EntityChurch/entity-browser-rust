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
//   * The WORKER BIN (`entity-worker.js` / `entity-worker_bg.wasm`) is
//     immutable-per-build but carries no hash in its URL, because trunk's
//     `data-type="worker"` pipeline emits fixed filenames. It is cache-first
//     keyed on the BUILD ID — the main bundle's hash, read out of index.html.
//     Same-build ⇒ zero bytes; new build ⇒ a miss, so it is fetched exactly
//     once per deploy. In Worker mode the worker IS the peer, so a stale one
//     is a stale runtime — which is why the key is the main bundle's hash and
//     not a timestamp: the two halves of the runtime cannot disagree.
//     (This was network-first until 2026-08-22, which meant a 17 MB
//     unconditional download on every load, once per worker. See WORKER_ASSET.)
//
//   * Everything else is MUTABLE at a stable URL and MUST be network-first
//     (falling back to cache only when offline):
//       - `/` and `index.html` — the app-shell entry. It names the hashed
//         bundles for THIS build; serving a stale copy pins the old
//         bundle (and re-ships already-fixed panics — see below).
//       - `entity-worker-loader.js` — 6 KB, and it is what carries the
//         `?log=` level, so there is nothing to win by caching it.
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

// The worker bin. Trunk's `data-type="worker"` pipeline emits FIXED filenames
// (the loader references them by a stable URL), so unlike the main bundle these
// two carry no identity in their path and miss HASHED_ASSET above.
//
// That put a **17 MB wasm on the networkFirst path, which fetches with
// `cache: 'reload'`** — a deliberate, unconditional, HTTP-cache-bypassing
// download. Correct for a 2 KB shell and catastrophic here: measured across
// three SW-controlled reloads of one page, the hashed 29.5 MB main bundle was
// fetched **0** times and `entity-worker_bg.wasm` **3** — once per load, and it
// multiplies by WORKERS, not pages (boot spawns one, every persisted `Backend*`
// peer respawns another). Every visitor paid it on every load, forever, and no
// HTTP cache could help because the fetch opts out of it.
const WORKER_ASSET = /^\/entity-worker(_bg\.wasm|\.js)$/;

// The build these bytes belong to, recovered from the main bundle's hash.
//
// We cannot key the worker on its own URL (it has no hash) and we must not key
// it on nothing (a cached worker running against a freshly-deployed main bundle
// is an `entity-wasm-worker-protocol` version mismatch — a WORSE failure than
// the re-download). Trunk does stamp the main bundle, and index.html names it,
// so the main bundle's hash IS this build's identity — and keying the worker on
// it makes the two impossible to disagree, which is strictly stronger than
// hashing the worker's own bytes would be.
//
// Deliberately not `Last-Modified`/`ETag` revalidation: `publish-serve`
// snapshots with `cp -a` (preserves mtime), so a validator-based scheme can be
// answered 304 for genuinely new bytes — the exact trap `cache: 'reload'` was
// introduced to avoid. A content hash cannot be fooled that way.
const BUNDLE_HASH = /entity-browser-([0-9a-f]{8,})(?:_bg)?\.(?:js|wasm)/;

// Cache key suffix carrying the build id. Not sent to the network — it only
// ever names an entry in the Cache API.
const BUILD_KEY_PARAM = '__entity_build';

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
    // the URL guarantees freshness). The worker bin → cache-first too, but
    // keyed on the BUILD id rather than on its own (hashless) URL. Everything
    // else (the mutable shell, the loader, sw.js) → network-first so a new
    // deploy reaches the browser on the first online load, falling back to
    // cache offline.
    if (HASHED_ASSET.test(url.pathname)) {
        event.respondWith(cacheFirst(req));
    } else if (WORKER_ASSET.test(url.pathname)) {
        event.respondWith(buildScopedAsset(req));
    } else {
        event.respondWith(networkFirst(req));
    }
});

// This build's id, or null when it cannot be established.
//
// Read from the cached shell first: `networkFirst` awaits its `cache.put` for
// navigations precisely so that by the time a page can spawn a worker, the
// shell in cache is the one that page is running. Falling back to a fetch
// covers the first-ever visit, where the navigation happened before this SW
// took control and nothing was cached.
async function currentBuildId(cache) {
    // EXACT match on the canonical key — deliberately not `ignoreSearch`, which
    // resolves in insertion order and would happily hand back a shell from a
    // previous build (see the note in `networkFirst`).
    let shell = await cache.match('/');
    if (!shell) {
        // Bounded (D23): this runs on the worker's boot path and has a defined
        // "could not establish the build id" outcome one line below — `null`,
        // which sends `buildScopedAsset` to network-first. A stall here would
        // wedge that path instead of taking the defined outcome, which is the
        // exact shape D23 forbids.
        shell = await fetchWithDeadline('/index.html', { cache: 'reload' }).catch(() => null);
        if (shell && shell.ok) await cache.put('/', shell.clone()).catch(() => {});
    }
    if (!shell) return null;
    const body = await shell.clone().text().catch(() => '');
    const m = BUNDLE_HASH.exec(body);
    return m ? m[1] : null;
}

function buildScopedKey(rawUrl, build) {
    const u = new URL(rawUrl);
    u.searchParams.set(BUILD_KEY_PARAM, build);
    return u.toString();
}

// Cache-first, keyed on (url, build id).
//
// A build id we do not recognise is a cache MISS, never a stale hit, so the
// dangerous direction — serving a worker from a different build than the main
// bundle — is unreachable by construction rather than by discipline. Entries
// for superseded builds are swept opportunistically; they are wrong to serve,
// not wrong to hold, so the sweep never has to win a race.
//
// When the build id cannot be established at all we fall back to the previous
// behaviour exactly (network-first). That is the fail-safe direction: it costs
// bytes, which is the failure we can afford.
async function buildScopedAsset(req) {
    const cache = await caches.open(CACHE_NAME);
    const build = await currentBuildId(cache);
    if (!build) return networkFirst(req);

    const key = buildScopedKey(req.url, build);
    const hit = await cache.match(key);
    if (hit) return hit;

    // DELIBERATELY unbounded, and this is the boundary of D23 rather than an
    // omission — see the same note on `cacheFirst`. There is no cached entry for
    // this build (that is what `hit` just ruled out) and an entry from a
    // DIFFERENT build is not a fallback but the protocol mismatch this whole
    // scheme exists to prevent. So a deadline here has nothing to fall back TO:
    // it would convert a slow 17 MB download into a hard failure. D23 bounds an
    // await that is blocking a defined alternative outcome; where the only
    // outcomes are "the bytes" and "nothing", waiting is correct.
    const fresh = await fetch(req, { cache: 'reload' }).catch(() => null);
    if (fresh && fresh.ok) {
        await cache.put(key, fresh.clone()).catch(() => {});
        dropSupersededBuilds(cache, req.url, build);
        return fresh;
    }
    // Offline with nothing cached for THIS build. An entry from another build
    // is not a fallback — it is the protocol mismatch this whole scheme exists
    // to prevent — so say so rather than booting a worker that cannot talk to
    // the main bundle.
    return offline503();
}

// Drop this asset's entries from every build except `keep`. Fire-and-forget:
// a stale entry is inert (nothing can match it once the build id moves), so
// failing or racing here costs disk, never correctness.
function dropSupersededBuilds(cache, rawUrl, keep) {
    const path = new URL(rawUrl).pathname;
    cache.keys().then((keys) => {
        for (const k of keys) {
            let u;
            try { u = new URL(k.url); } catch (_) { continue; }
            if (u.pathname !== path) continue;
            if (u.searchParams.get(BUILD_KEY_PARAM) === keep) continue;
            cache.delete(k).catch(() => {});
        }
    }).catch(() => {});
}

// Cache-first for immutable hashed assets, with a background revalidate as
// a belt-and-suspenders refresh (the URL changing per build is the real
// invalidation; this just heals a corrupted/partial cache entry).
async function cacheFirst(req) {
    const cache = await caches.open(CACHE_NAME);
    const cached = await cache.match(req);
    if (cached) return cached;

    // DELIBERATELY unbounded. A hashed asset that is not in the cache has no
    // fallback — there is no older copy to serve, because the URL IS the
    // version. A deadline would turn a slow first download of the ~30 MB main
    // bundle into a hard failure on exactly the connections least able to
    // afford one, and would gain nothing: the alternative to waiting is a 503.
    //
    // The user-visible consequence of a stall here is a first load that does not
    // finish, which a reload retries (E1). That is categorically different from
    // the networkFirst case, where a perfectly good cached shell was sitting
    // unreachable behind an await that never returned (E6). **D23 is a rule
    // about awaits that block a defined alternative, not a rule about the word
    // `fetch`** — and stating the boundary here is what stops the next reader
    // from "fixing" this line.
    const fresh = await fetch(req).then((resp) => {
        if (resp && resp.ok) cache.put(req, resp.clone()).catch(() => {});
        return resp;
    }).catch(() => null);
    if (fresh) return fresh;
    return offline503();
}

// How long the network leg of `networkFirst` may take before the cached copy
// wins. **3000 ms, borrowed rather than invented:** Workbox — the reference
// implementation of this exact strategy — ships `networkTimeoutSeconds: 3` for
// navigations in its `pageCache()` recipe. Its documented rationale is verbatim
// our symptom: without a timeout, network-first "will wait indefinitely for a
// network response even when the user is offline, only falling back to cache
// after the connection eventually times out (which can take 30-60 seconds),"
// leaving "loading spinners spinning endlessly."
//
// Keep this in step with `BOOT_FETCH_DEADLINE_MS` in `src/net.rs`. They are two
// tiers of the same discipline (D23) and a user hitting one hits the other.
const NETWORK_DEADLINE_MS = 3000;

// `fetch`, but it gives up. This is the whole of C2, and it exists because we
// hand-rolled network-first and omitted the one option its reference
// implementation considers essential (AP28).
//
// **The case it covers is not "offline".** A network that REJECTS — interface
// down, DNS failure, connection refused — rejects this promise promptly, the
// caller reaches its fallback, and offline reload works. It always did; that is
// why the freeze was intermittent and looked like a fluke. A network that
// ACCEPTS AND NEVER ANSWERS — a captive portal, a half-open socket, a foreign
// LAN blackholing an address that used to work, an overloaded CDN edge — never
// rejects. The cached shell sits one line below the caller's `await` and is
// only consulted in the `.catch`, which is never reached. The page is blank for
// as long as the OS is willing to wait. Brick-matrix cell #2, and unlike the
// cold-boot one it applies to production HTTPS on any flaky network.
//
// AbortController rather than a bare `Promise.race`: racing would leave the
// request in flight, still holding a connection and still able to write into
// the cache after we had already decided to serve the cached copy — a
// last-writer-wins race against ourselves. Aborting ends it.
//
// Gate: `a_cached_shell_survives_a_blackholed_origin` (G1/SW), which was
// observed red on the unfixed worker before this function existed.
function fetchWithDeadline(req, init) {
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort('entity-browser: network deadline'), NETWORK_DEADLINE_MS);
    return fetch(req, Object.assign({}, init, { signal: ctl.signal })).then(
        (resp) => { clearTimeout(timer); return resp; },
        (err) => { clearTimeout(timer); throw err; }
    );
}

// Network-first for the mutable app shell + non-hashed worker bundle:
// always prefer the freshly-deployed bytes when online; fall back to the
// last-cached copy offline so the peer still boots; else a readable 503.
//
// `cache: 'reload'` is load-bearing: a plain `fetch(req)` respects the
// browser's HTTP cache, so a reload sends a CONDITIONAL request and the
// server can answer 304 — at which point `fetch` resolves with the browser's
// STALE cached copy and network-first silently serves the old shell (this is
// exactly the "I refreshed but got the old build / a 304" symptom). Worse,
// `publish-serve` snapshots with `cp -a` (preserves mtime), so even freshly
// rebuilt bytes can keep an old mtime and 304. `reload` forces an
// UNCONDITIONAL network fetch (no If-Modified-Since), so we always get the
// true latest bytes on the first online reload — dev server or CDN alike.
// Is this navigation a request for the ORIGIN'S OWN shell — the thing `/`
// serves — as opposed to some other navigable document?
//
// Until C9 there was only one navigable document and the query string was the
// only thing that varied, so "a navigation" and "`/`" were the same statement.
// C9 retains shells at `/builds/<build_id>/index.html`, and C10 navigates to
// one to honour a rollback pin. Those are DIFFERENT DOCUMENTS: caching them
// under `/` makes the canonical offline shell the rolled-back build, which then
// outlives all three of the pin's ways out (TTL, self-clear, attempt counter),
// because each of those only runs on a load of `/` and offline `/` is served
// from that entry. `currentBuildId` reads it too, so the build id a page
// reports offline would be the rolled-back one.
//
// `/` caches what `/` serves. A retained shell is cached under its own URL by
// the `else` branch, which also makes a pinned build work offline — it did not
// before, since its navigation was stored under a key it never requests.
function isCanonicalShell(req) {
    try {
        const p = new URL(req.url).pathname;
        return p === '/' || p === '/index.html';
    } catch (_) {
        // Unparseable URL: fall back to the old behaviour rather than losing the
        // shell entry entirely. A shell cached under `/` is the recoverable
        // mistake; no shell at all is the offline-503 one.
        return true;
    }
}

async function networkFirst(req) {
    const cache = await caches.open(CACHE_NAME);
    const fresh = await fetchWithDeadline(req, { cache: 'reload' }).then(async (resp) => {
        if (resp && resp.ok) {
            if (req.mode === 'navigate' && isCanonicalShell(req)) {
                // A navigation TO THE ORIGIN'S OWN SHELL is cached under the
                // CANONICAL `/`, never under its own URL, and the put is AWAITED.
                //
                // **`isCanonicalShell` is not decoration — see its comment.** This
                // rule was written when every navigation WAS `/` (only the query
                // string varied). C9 added a second navigable document, and folding
                // a retained shell into `/` poisons the offline shell with the
                // rolled-back build.
                //
                // Both halves are load-bearing and the first one cost a caught bug.
                // `buildScopedAsset` reads this build's id out of the cached shell; an
                // earlier version stored navigations under their full URL (`/?worker=1`)
                // and read them back with `cache.match('/', {ignoreSearch: true})`, which
                // matches in INSERTION order — so it kept returning the `/` entry written
                // once at install and never updated. The build id could then never move,
                // and a deploy served the OLD worker to the NEW bundle: precisely the
                // protocol mismatch this scheme exists to prevent, and silent. One key,
                // always rewritten, removes the ambiguity instead of ordering around it.
                //
                // Awaiting it is what makes "which build is this page running" an ordered
                // fact rather than a race: a page cannot spawn a worker before it has
                // received its own navigation response. A ~50 KB document is not worth a
                // rule that only sometimes holds.
                //
                // Nothing is lost by dropping the per-query entry — the offline path
                // below already falls back to `/` with `ignoreSearch`, which is how
                // `?systemrecovery=1` boots offline — and it stops the cache growing an
                // entry per distinct query string.
                await cache.put('/', resp.clone()).catch(() => {});
            } else {
                cache.put(req, resp.clone()).catch(() => {});
            }
        }
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

// Entity Browser — the SERVICE WORKER KILL SWITCH (C17).
//
// This file is not part of the running application and nothing registers it.
// It is the recovery artifact for **brick-matrix cell #10**: a bad `sw.js` that
// breaks navigation itself. In that state the page does not load, so no in-page
// affordance can help — not the app, and not the `?systemrecovery=1` console,
// which arrives over the same channel the worker has poisoned.
//
// ## How to use it (the runbook lives in docs/RUNBOOK-SERVICE-WORKER-KILL-SWITCH.md)
//
// Serve THESE BYTES at `/sw.js`. Not at this filename — at the URL the broken
// worker is registered under. A returning browser fetches `/sw.js` out of band
// on navigation (`updateViaCache: 'none'`, and Chrome ignores the HTTP cache for
// the worker script entirely), finds different bytes, installs this, and this
// takes itself and every cache down.
//
// ## Why this is the only mechanism
//
// **Deleting `sw.js` does not work.** The spec deliberately does not unregister
// on a failing response — 410, 404 and a bad MIME type are all consistent with
// each other and none of them retire a registration. A server-side kill-switch
// header was proposed (w3c/ServiceWorker#614) and never built. So the only way
// out is to serve a worker that removes itself, and that only works **while you
// can still serve a file at that URL**. If `sw.js` is itself cached long at the
// CDN, the kill switch is unreachable for that TTL and the floor is the spec's
// 24-hour staleness cap. That is the real RTO for a worker-tier brick; it is
// written down rather than hoped away.
//
// ## Three things this file has to get right
//
// 1. **`skipWaiting` in `install`.** Without it this worker waits for every
//    controlled tab to close — and the tabs are broken, so the user's instinct
//    is to leave them open and keep reloading.
// 2. **`clients.navigate` in `activate`.** Unregistering does not release the
//    pages the old worker is already controlling; they stay controlled until
//    they navigate. A tab the user is staring at right now must be reloaded
//    *for* them, or the fix only reaches people who happened to close it.
// 3. **Every cache deleted, not just ours.** A broken worker may have written
//    caches under names this project does not know. `caches.keys()` is the only
//    honest enumeration.
//
// **This is explicitly lossy, and that is the accepted trade.** It removes the
// offline shell: a visitor with no network afterwards gets nothing until they
// are online once. The spec participants' own framing applies — collateral
// damage is acceptable because *it is a kill switch*. It touches **no** peer
// data: IndexedDB, OPFS and localStorage are not caches and are not ours to
// clear from here (the same scope line the recovery console's program reset
// draws, and for the same reason — there is still no export path).
//
// ## Keep it deployed
//
// Not until the fleet looks healthy — for a long time. Stale clients check in
// weeks later, and a client that has not been opened since the bad deploy is
// exactly the one that still needs this.

self.addEventListener('install', () => {
    // Do not wait for the broken worker's clients to close. They are broken;
    // nobody is closing them.
    self.skipWaiting();
});

self.addEventListener('activate', (event) => {
    event.waitUntil(
        (async () => {
            // Order matters only in that all three must happen; each is
            // independently guarded so one failure cannot strand the others.
            try {
                const keys = await caches.keys();
                await Promise.all(keys.map((k) => caches.delete(k).catch(() => {})));
            } catch (e) {
                // Nothing to report to — there is no page listening yet.
            }

            try {
                await self.registration.unregister();
            } catch (e) {
                /* fall through: still try to release the tabs */
            }

            // Unregistering does not release an already-controlled page. Reload
            // the tabs the user has open, or this only reaches the ones they
            // closed.
            try {
                const clients = await self.clients.matchAll({ type: 'window' });
                for (const client of clients) {
                    try {
                        await client.navigate(client.url);
                    } catch (e) {
                        /* a client that refuses navigation is not fatal here */
                    }
                }
            } catch (e) {
                /* ignore */
            }
        })()
    );
});

// **No `fetch` handler, deliberately.** A worker with no fetch listener does not
// intercept anything: every request goes straight to the network, which is the
// state we are trying to get back to. Adding a pass-through handler here would
// be strictly worse — it would put this worker in the request path for the short
// window before it unregisters.

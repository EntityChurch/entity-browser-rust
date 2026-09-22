# RUNBOOK — the service-worker kill switch

**When to reach for this:** a deployed `sw.js` is breaking the application for
returning visitors, and the ordinary hotfix path is not reaching them.

**What it costs:** the offline shell, for everyone who runs it. Nothing else.
It does not touch the entity tree.

**What it does not fix:** anything that is not the service worker. If a bad
*build* is the problem and the worker is fine, a new deploy reaches browsers on
the next load — that is the property
`a_new_build_reaches_a_browser_the_service_worker_already_controls` gates, and
this runbook is the wrong tool.

---

## 0. Decide which failure you have

There are two, they look alike from a bug report, and only one of them needs
this page.

| | The page loads | Reach for |
|---|---|---|
| **A bad worker, page still loads** | yes | `?systemrecovery=1` → *Reset the cached program*. The user can do it themselves, on a phone, in about ten seconds |
| **A worker that breaks navigation itself** | **no** | this runbook |

The second one is the reason this exists: nothing in the page can help, because
nothing in the page arrives. That includes the recovery console — it comes over
the same channel the worker has poisoned.

**Tell them apart by asking a reporter to open `/?systemrecovery=1`.** If the
console renders, you are on row one.

---

## 1. Confirm you can still serve `/sw.js`

This is the whole precondition, and it is the one that can be false.

```
curl -sSI https://<domain>/sw.js
```

You need a `200` and a `Cache-Control` that revalidates. If `sw.js` is sitting
behind a long immutable TTL at the CDN, **the kill switch is unreachable until
that TTL drains** — purge the CDN first, and if you cannot, the floor is the
spec's 24-hour worker staleness cap. That cap is the real RTO for this class of
incident. It is not improvable from here.

**Do not delete `sw.js` instead.** A 404, a 410 and a wrong MIME type all leave
the registration in place — the spec deliberately does not unregister on a
failing response, and the server-side kill-switch header that would have fixed
this was proposed in 2015 and never built. Deleting the file removes your only
way out.

---

## 2. Serve the self-destruct worker at `/sw.js`

The artifact ships with every build, so there is nothing to compile:

```
# at the origin, from the deployed tree
cp sw.js sw.js.broken            # keep the evidence; you will want it later
cp sw-selfdestruct.js sw.js
```

Then purge `/sw.js` at the CDN.

Source of truth is `assets/sw-selfdestruct.js` in this repo. It is three things
and each is load-bearing: `skipWaiting()` on install (the broken tabs are open
and nobody is closing them), every cache deleted by enumeration rather than by
our own names (a broken worker may have written caches we do not know about),
and `clients.navigate()` on activate (unregistering does **not** release an
already-controlled page — a tab someone is staring at right now must be reloaded
for them).

---

## 3. Verify on a device that is already broken

Not on a fresh profile. A fresh profile has no registration and will look fine
whatever you deploy — that is the vacuous pass this whole section exists to
avoid.

On a browser that currently has the bad worker:

1. Load the site. The tab should reload itself once, on its own.
2. `navigator.serviceWorker.getRegistrations()` → `[]`.
3. `caches.keys()` → `[]`.
4. The application boots.

If step 1 does not happen, the browser has not fetched `/sw.js` yet — step 1 of
this list *is* the fetch, so give it one more navigation before concluding
anything.

---

## 4. Leave it deployed

**Not until the fleet looks healthy — for weeks.** A client that has not been
opened since the bad deploy is exactly the one that still needs this, and it
will not check in until someone opens it. Removing the kill switch early means
the stragglers arrive at an origin that no longer has a cure.

While it is deployed the site has **no offline shell**. That is the accepted
trade; say so in the incident notes rather than discovering it as a second
incident.

---

## 5. Come back off it

When you are ready to restore normal caching, deploy the real `sw.js` again. It
installs over the self-destruct worker the same way the self-destruct worker
installed over the broken one — there is nothing special about the return trip.

---

## 6. The drill

`make e2e-worker T=the_kill_switch` rehearses exactly the sequence above against
a staged copy of the SPA: install a worker that breaks navigation, **assert the
page is genuinely broken**, serve the self-destruct worker at `/sw.js`, and
assert the browser recovers on its own.

**Never rehearsed means it does not exist.** The gate is what makes this page a
capability rather than a plan, and the assertion that the break was real is what
keeps the gate from passing on a browser that was never broken in the first
place.

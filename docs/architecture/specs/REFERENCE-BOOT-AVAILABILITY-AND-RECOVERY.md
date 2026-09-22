# Reference — boot, updates, and recovery

How Entity Browser starts, how a new build reaches a browser that has already visited, what
happens when something goes wrong, and the small number of things a deployment must get right
so that all three keep working.

Read this if you are **deploying Entity Browser on your own domain**, or if you need to
understand why a visitor is seeing an older version than you published. It pairs with
*Publish a site — the quickstart*, which covers getting content onto a domain; this document
covers the application shell that renders it.

**Scope note.** This describes the browser deployment. The Tauri desktop build is distributed
as an installer and none of the update or recovery machinery below applies to it.

---

## 1. The two guarantees

Everything here serves two properties, in this order.

**A reload always reaches a bootable application.** Not necessarily the newest one — *bootable*.
Getting the newest build to a visitor is the goal; leaving a visitor with nothing at all is the
failure that has no exit, because a browser application can only be repaired by shipping it code,
and it can only receive code if it can still start.

**Updating is silent and automatic.** A visitor is never asked which version to run, never
prompted to clear anything, and never needs to know that a version identifier exists. The
recovery surfaces described in §5 exist for the case where something has already gone wrong;
they are not part of normal use.

---

## 2. What a deployment consists of

An Entity Browser deployment is two kinds of file, and the distinction is the single most
important thing on this page.

**Entry files** are named by a stable URL and their contents change with every release:

| | |
|---|---|
| `/` and `/index.html` | the application shell. Names the exact bundle files this release uses |
| `/sw.js` | the service worker |
| `/entity-worker-loader.js` | the worker bootstrap |
| `/entity-deployment.json` | per-domain configuration, if you serve one |

**Asset files** are named by a hash of their own contents — `entity-browser-<hash>_bg.wasm` and
similar. A given asset URL's bytes never change. A new release produces new asset *names*; it
never rewrites an existing asset.

Two rules follow, and both are load-bearing:

> **Entry files must be revalidated on every request.** `Cache-Control: no-cache`, or a short
> `max-age` with `must-revalidate`. **Never `immutable`, and never a long `max-age`.**
>
> **Asset files may be cached indefinitely** — `max-age=31536000, immutable` is correct and
> desirable.
>
> **Never delete the assets of a release that a visitor might still be running.** Retain
> previous releases' assets for a generous window.

### 2.1 Why the entry rule matters more than it looks

The shell is what names the bundles. If a browser or a CDN serves a stale shell, it requests the
*old* bundle names, and the new code is never fetched — the release is invisible no matter how
correctly it was published. A visitor in this state cannot be repaired by publishing again,
because publishing does not reach them.

`Cache-Control: immutable` means precisely what it says: the response may be stored for a year
and **never revalidated**. Purging your CDN does not reach a browser that already holds such a
response. There is no remedy for those clients except serving the application from a URL they
have never seen, or waiting out the lifetime you told them to use.

**This is the one deployment mistake with no clean recovery.** Everything else on this page is
survivable.

### 2.2 Classifying your own content

If you publish content alongside the application, classify by *whether the bytes at that URL can
change*, not by file type or by directory name. A directory called `content/` is not by itself
evidence of immutability — several static-site generators use that name for source files that
are rewritten in place on every build. A path is safe to mark immutable only when something
about the URL — a content hash in the name — guarantees the bytes cannot change.

Getting this wrong in the safe direction costs bandwidth. Getting it wrong in the unsafe
direction produces §2.1.

---

## 3. How an update reaches a returning visitor

1. The visitor loads the page. The shell is revalidated against the network, so they receive the
   newest shell on the **first online load** after a release.
2. The new shell names new asset URLs. Those are fetched and stored permanently; assets from the
   previous release remain stored and are simply no longer referenced.
3. If a service worker is active (§4), it also updates itself. The worker script is always
   checked against the network and is never satisfied from the HTTP cache.

**A visitor gets a new release by reloading, or by closing the tab and returning.** No cache
clearing, no private window, no manual step. If that is not what you observe, the cause is
almost always an entry file being cached against the rule in §2 — at your CDN, or by a
`Cache-Control` header you did not intend.

### 3.1 An unresponsive network delays a load; it does not prevent one

Worth stating because the failure it describes is easy to misread as the application being
broken, and because the behaviour changed.

There is a difference between a network that **refuses** a connection and one that **accepts
it and then never answers**. The first is what happens when a machine is offline or a name
does not resolve, and it fails immediately. The second is what a captive portal does before
you have signed in, and what a network does when it is silently dropping traffic to an
address that used to work — a laptop carried from one wifi network to another, for instance.
Nothing fails; the request simply never completes.

Every network read the application performs *before it can paint* now carries a deadline of a
few seconds, and passing that deadline is treated as an answer rather than as something to
keep waiting for:

- **Per-domain configuration** that does not arrive in time is treated as not being served,
  and the application starts on its built-in defaults.
- **The application shell**, when a service worker is active and has a copy stored, is served
  from that stored copy once the network has had its few seconds.

So a visitor on an unresponsive network sees the application start a few seconds slower,
possibly with configuration from a previous visit rather than the current one. Previously such
a visitor saw a blank page for as long as their operating system was willing to wait — which
behind a captive portal is indefinitely — with no message and no indication that anything was
wrong.

**A first-ever visit is the exception, and it cannot be otherwise.** A browser that has never
loaded the application has nothing stored to fall back to, so on an unresponsive network it
will not start. Reloading once the network is working is the remedy, and no deadline can
manufacture a copy that was never downloaded.

---

## 4. The service worker, and when it does not exist

**A deployment ships two independent programs, and it is worth being precise about which is
which**, because they update on separate schedules and a symptom in one is often blamed on the
other:

- **The application** — `entity-browser-<hash>_bg.wasm`, tens of megabytes. Entity Browser
  itself. Its filename contains a hash of its own contents, so a new release is a new URL.
- **The service worker** — `sw.js`, about 15 KB, and **not part of the application**. A small
  network proxy the browser runs on its own thread, outside the page. It persists after the tab
  is closed, and every request the application makes passes through it. It fetches the
  application; the application does not know it is there.

The service worker serves hash-named assets from local storage, and revalidates entry files
against the network, falling back to the stored copy when the network is unavailable.

`sw.js` is deliberately **not** given a hashed filename, and cannot be. A browser registers a
service worker by URL and updates it by re-fetching that exact URL and comparing the bytes. If
the filename changed per release, pointing at a new one would mean editing `index.html` — which
is served by the *old* service worker. A stale shell would then make the worker impossible to
replace: you would need a working service worker in order to update the service worker. The
stable URL is what makes the worker replaceable, so `/sw.js` must be served under the entry-file
rule in §2.

**Browsers only permit service workers in a secure context.** In practice that means **HTTPS**,
with `http://localhost` and `http://127.0.0.1` as the only exceptions.

The consequence is worth stating plainly, because it surprises people:

> **Served over plain HTTP on anything other than `localhost` — a LAN address such as
> `http://192.168.1.20:8081`, for instance — no service worker is registered, nothing is stored,
> and the application has no offline capability at all.** A reload while the server is
> unreachable will not fall back to a cached copy, because there is no cached copy. The browser
> simply waits for a server that is not answering.

This is a property of the web platform, not a configuration option. If you want to exercise
offline behaviour, or reach a development build from a phone or a second machine and have it
behave the way production does, you must serve it over HTTPS — a self-signed certificate is
sufficient.

To check whether a service worker is active for an origin: browser developer tools →
Application → Service Workers. An empty list means everything in this section does not apply to
that origin.

---

## 5. When something goes wrong

**System Recovery** is a diagnostic console that runs *instead of* the application, reachable at
`?systemrecovery=1`. It is deliberately minimal: plain JavaScript, no application code, no
storage opened and no peer started, so that it remains usable when the application itself cannot
start. It reports what is stored locally — categorised into application code, your data,
credentials, and transient state — and it performs no writes and no deletions.

It is reachable offline, and it is safe to open at any time.

**What it is for:** determining whether your data is present when the application will not
start. It answers *"is my data there?"* — a question worth separating from *"is my data
correct?"*, which it does not attempt.

### 5.1 Current limits, stated plainly

This is a research preview and the recovery story is incomplete. As of this release:

- **There is no export.** Nothing exports a tree, a site, or a peer. The universal repair —
  clearing site data for the origin — therefore destroys everything stored, including work
  unrelated to whatever is being repaired. Treat it as a last resort.
- **There is no version selection.** A browser runs whichever release the shell names; there is
  no supported way to run a previous one.
- **The application does not report which build it is running**, so a bug report cannot easily
  identify a version.
- **The "a new version is available" notice is raised when `sw.js` changes, not when the
  application changes.** Since `sw.js` changes far less often than the application does, most
  releases produce no notice. This affects only the *prompt*: the shell is revalidated on every
  load, so a reload still lands the new release whether or not a notice appeared.
- **A tab left open for a long time does not re-check for updates.** The update check runs at
  page load; there is no periodic re-check. Reload, or reopen the tab, to pick up a release.
- **A configuration value learned from a domain is not re-checked** in every case. If a domain's
  publishing identity changes, a browser that visited before the change may continue addressing
  the previous one, and the resulting failure looks like missing or stale content rather than an
  error.

These are known and are being worked on. They are listed here rather than omitted because each
one changes what a deployer should do when a visitor reports a problem.

---

## 6. Deploying safely — the short version

1. Publish assets first, then entry files. Never the other way round.
2. Never delete assets belonging to a release a visitor might still be running.
3. Verify your cache headers **after** deploying, by fetching the live URLs with a GET — not by
   reading your configuration, and not with a HEAD request, which some CDNs answer without
   consulting or populating their cache. Confirm that every entry file in §2 revalidates and
   that none is marked `immutable`.
4. Serve over HTTPS. §4.
5. Keep the previous release's files in place long enough that a visitor mid-session cannot be
   left requesting a file that no longer exists.

---

## 7. Related

- *Publish a site — the quickstart* — identity, content, build, verify, upload, republish, and a
  worked CDN recipe.
- *Deployment and configuration guide* — per-domain configuration and startup surfaces.
- *System Vision* and *Project Architecture* — what the application is and how it is built.

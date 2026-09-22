# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A **tag is a release** — a push to `master` that changes nothing you would
version-pin against doesn't get one (ADR-0015). Each release's notes on GitHub
are generated from that version's section below, so write it for the person
downloading the artifact.

## [Unreleased]

Nothing yet.

## [0.10.0] — 2026-09-20

Two themes, in the order they were built.

The first is what a research preview earns the release after its first big one:
**what happens when a deployment goes wrong.** `0.9.0` could publish a site,
connect two peers and move a file between them. It could also — these observed
rather than reasoned about — serve a stale app indefinitely because a copy was
already on disk, blank the screen for the whole of a slow boot with the recovery
hatch hidden behind it, and put a returning reader back on the front page having
silently discarded where they were. And structurally, with no incident needed to
prove it, a bad service worker could pin a broken build in a visitor's browser
with no way out that did not involve developer tools — which is not an incident
but a brick, and is unavailable on the device most stuck visitors are holding.

Every one of those is fixed here, and the ones that cannot be prevented now have
a stated recovery path rather than an implied one.

The second theme arrived once that work was done: **the tree is something you
write to, not only something you read.** `0.9.0` could publish a site from a
directory and show it to someone else. This release adds a **feed** — you follow
publishers, read what they posted, and post yourself from inside the app, where
the write into your own tree *is* the publish — and three windows that make the
device you are on legible: what files are here, what is using the space, and
what this tab is spending its time on.

Still a **research preview**.

### Added — recovery, when the deployed build is the problem

- **A bad build has somewhere to fall back to.** `entity-browser builds <DIR>`
  (or `make builds-manifest`, run automatically by `make site-dist`) retains each
  published shell at `/builds/<build-id>/index.html` and writes a `/builds.json`
  listing them. `?build=<id>` boots a retained one. A build is identified by its
  **bundle hash**, not the commit — two docs-only commits produce byte-identical
  code and are one build, which is what a rollback target has to mean.
- **The pin is a lease, not a deed.** A rollback a visitor cannot fall *out* of
  is a brick with better manners, so it expires on a TTL, clears itself when the
  live origin stops serving what it rolled away from, and heals via an attempt
  counter if the build it names is missing — which is the case that matters,
  because at a 404 none of our code runs to fix anything. The recovery console
  is exempt from redirection, since it is the surface that can clear a pin.
- **A last-resort kill switch for a service worker that breaks navigation.**
  `assets/sw-selfdestruct.js` ships with every build and is registered by
  nothing; served at `/sw.js` during an incident it unregisters the bad worker
  and navigates the pages it controls back to a working app. This is the only
  mechanism that exists — the spec deliberately does not unregister on a failing
  response, so deleting `sw.js` (404, 410, wrong MIME type) leaves a bad worker
  installed. Runbook: `docs/RUNBOOK-SERVICE-WORKER-KILL-SWITCH.md`. It is lossy
  by design: it removes the offline shell and touches no peer data.
- **The recovery console can now act.** `?systemrecovery=1` previously
  enumerated service workers and cache storage and could do nothing about
  either — its printed advice was *"use your browser's developer tools"*, which
  is not advice on a phone, where a stuck visitor usually is. It has one
  confirm-gated action, **Reset the cached program**, scoped to the two stores
  holding app code. It does not touch your entity tree or local storage, and it
  reports what it observed afterwards rather than what the API returned, so
  *"there was nothing to remove"* cannot render as *"fixed"*. Its own copy used
  to tell you that clearing site data *"only removes the cached program"*, which
  was false and would have destroyed your data; that is corrected.
- **The console can name your version and who you are pointed at.** It reports
  the build the profile is running and the publisher it is routed to, separately
  from who actually publishes the site — the distinction that turns *"the app is
  broken"* into a diagnosis. This tier runs before the app boots, deliberately,
  because anything that needs the app to start in order to escape a build that
  will not start is not a recovery mechanism.

### Added — everything new here is in 30 languages

- **The health checks, the recovery copy and the insecure-origin warning are
  translated**, so the surfaces you reach when something is wrong speak the same
  language as the rest of the app. Localization is 30 locales × **1002**
  strings, up from 702 at `0.9.0` — the feed, the three new windows and the
  status bar are all translated, not English-only surfaces bolted on.
- **The insecure-origin warning now names all three things you lose**, not two:
  it previously mentioned background storage and the camera and omitted the
  offline shell, which is the one a person actually notices — it is why a phone
  loading the app from a desktop over plain `http` cannot open it again once the
  desktop sleeps.

### Added — the app checks its own health

- **A *Problems* card in System Overview**, with three checks: who publishes
  this site, whether a publisher is still answering, and whether everything
  finished loading. A check that **could not run** is put in front of you rather
  than counted as healthy — *"I could not check"* rendering as *"you are fine"*
  is the failure mode a health surface exists to avoid.
- **Some findings offer a remedy**, and no remedy is destructive — there is no
  export path yet, so nothing here may cost you data to repair. A remedy reports
  what it actually did, including *"nothing was listening"* when the window that
  would carry out the retry is not open.

### Added — publisher and operator tooling

- **`make fleet-probe`** — one command answers what is deployed across your
  domains and, critically, whether it can be *fixed*: it checks that every
  mutable URL (`/`, `/index.html`, `/sw.js`, `/entity-deployment.json`) is served
  revalidating rather than pinned, which is what decides whether a hotfix can
  reach anyone. It judges uniformity by bundle hash, printing the commit label
  beside it rather than as the verdict.
- **`make serve TLS=1`** serves the dev tree over HTTPS with a locally-trusted
  certificate. The service-worker and offline tiers only exist on a secure
  origin, so before this they were not testable off `localhost` at all.
- **`make serve DIST=<dir>`** now honours the directory you pass it. It silently
  served `dist/` regardless — including when `make site-dist` printed the
  `DIST=dist-site` invocation as its own closing advice, so the one command for
  reviewing the uploadable tree served the wrong one, and the two trees differ in
  exactly what you would be comparing them for.

### Fixed — boot

- **The screen was blank for the whole of a slow boot, and the escape hatch went
  down with it.** The loading surface was taken down before the application tier
  started rather than when something was ready to replace it, so a boot behind a
  slow or unreachable origin showed nothing at all — with the always-visible
  *Open System Recovery* hatch, which lives inside that surface, gone at the
  moment it was needed. The surface now comes down when the replacement is live.
- **A slow origin no longer holds up the whole boot.** Boot is two phases: local
  reads happen first and the app's frame loop goes live behind them, and
  everything depending on the network read happens after, behind a painted page.
  A phase that never reports back cannot hold the page hostage — a failsafe hands
  it over regardless. You see exactly one transition, the same one as before.
- **Network reads on the boot path are bounded.** An origin that accepts a
  connection and never answers — the case a static file server cannot even
  simulate — used to hang boot indefinitely. Every boot-path fetch now has a
  deadline, in both the app and the service worker.
- **A profile with a local home stopped reading its domain's configuration
  entirely**, which withheld origins, publisher-change detection and every log
  line about the document from exactly the profiles most likely to need them. The
  document is read unconditionally now; only whether it is *adopted* depends on
  your setting.
- **"Is there a deployment configuration?" is no longer one answer.** A domain
  that serves none on purpose, one that answered with a fault, one that is
  unreachable, and one whose document could not be parsed are four different
  facts, and the log says which. A 502 is not a deployer choosing to serve no
  configuration.

### Fixed — your session comes back

- **A reload put you back on the deployment's front page.** Every window read its
  saved state synchronously at construction — which, in the storage mode the
  browser build actually ships, reads from a cache that has not filled yet. So a
  returning reader was silently put back on the build default and shown *"No site
  manifest at 'demo'"* for a site that was there. Measured on the shipped mode as
  an intermittent 1-in-3, and on the opt-in Worker mode as every time. All eight
  affected surfaces now read the durable tree.
- **A Shell window did not merely forget its state — it overwrote it.** Opening
  one discarded the persisted working directory, command history and draft.
  That was data loss rather than lost session state, and unrecoverable after the
  fact.
- **Windows now return to their own slots, whatever order you reopen them in.**
  Window ids restart at 1 each session, so window 2's saved state was whatever
  window 2 was *last* time — of any type. There is now an index recording which
  window each saved entity belongs to, and every decoder checks what wrote a slot
  before adopting it. Previously, measured, the Entity Tree adopted the Knowledge
  Base's expanded-path list.
- **A Shell after a reload rendered nothing.** Re-running the last command
  produced a byte-identical saved entity, and the window was waiting on a change
  to that entity to redraw — so the output existed and was never painted.

### Fixed — content, publishers and caches

- **Opening a publisher you had just looked up made them unreachable, for good.**
  Look a name up in the Registry Browser, press *Open*, and — on a domain that
  serves its own publisher, which is the ordinary arrangement — that publisher
  vanished from the whole profile. Their feed reported that the deployment did
  not know where they were hosted; the site list went from their sites to *"No
  sites yet"*, on the same load that had just fetched every one of those sites'
  manifests. The content was on your device the entire time; what was lost was
  the record of **where** the publisher lives. Reloading did not help, and
  nothing in the application would ever have repaired it: the record is treated
  as a choice *you* made, so the deployment's own declaration is deliberately
  not allowed to overwrite it.

  The cause was one comparison. An empty origin means *the same place this
  application is served from* — it is what a single-domain deployment publishes
  — and one reader treated it as *no origin recorded at all*, so the record was
  written and then could not be seen. Three distinct situations now stay
  distinct: nothing recorded, something recorded we cannot read, and an origin
  recorded as *here*.
- **A republished app kept serving the old bytes, indefinitely.** Any copy of a
  publisher's content held on disk was treated as current merely because it was
  present, so no request was issued and no cache anywhere downstream got a
  chance to be right. Every foreign artifact now goes through one freshness
  check, which is a 58-byte pointer fetch in the common case. An origin that
  cannot be reached leaves the held copy alone — a cache that drops what it
  cannot re-verify turns an outage into a missing app.
- **A publisher that changes identity now heals on the next load.** A deployment
  that re-keys used to leave every stored reference pointing at the retired
  publisher. The record of a change is also re-checked against the domain and
  dropped when the domain contradicts it, so an ordinary mistake at the source
  does not become permanent state in your browser.
- **A home site you chose was overwritten by the deployment's declaration.** If
  you picked a cached site from another publisher as your home, the next boot
  replaced it *and* wrote down a durable record naming your own choice as
  retired, which then rewrote every stored reference to it. A value the
  deployment seeded and a value you deliberately chose are byte-identical in
  storage; they are now marked, and yours wins.
- **Publishing a second publisher to a domain destroyed the first one's site.**
  Where several publishers share one domain — told apart by their trees rather
  than by separate hosting prefixes — each publish cleaned the whole shared area
  rather than its own part of it, deleting the earlier publisher's signature and
  its published pages. The tree directory was left behind, so the output still
  looked right; running the publisher's own `--verify` on the earlier publisher
  reported that nothing in it could be resolved. A publish now cleans only its
  own publisher's tree and pages, and never removes shared content — surplus
  files can be swept later, whereas another publisher's signature cannot be
  brought back.
- **Reading one publisher could follow another publisher's address layout.** The
  small file a publisher ships to describe where its content lives is shared by
  everyone at the same location, so where two publishers shared one, the second
  one's description was used to look for the first one's pages — reporting a
  perfectly intact site as unresolvable. A description that names a different
  publisher is no longer treated as authoritative; one that names nobody still
  is, since not every publisher includes it.
- **Publishing a second peer to a domain erased the first one from that domain's
  configuration.** A domain can host several publishers, each under its own
  `--prefix`, and its `/entity-deployment.json` is meant to name all of them.
  Instead each publish overwrote the file with a single-peer document, so the
  earlier peer's serving origin vanished and the domain's home site moved to
  whoever published last — which, for anyone who had already visited, also
  recorded the previous publisher as *retired* and silently redirected every
  saved reference to it, while that publisher was alive and serving. Publishing
  now merges: the home publish owns the domain-level settings, a secondary
  publish adds only its own entry, and `--set-home` is how you deliberately move
  a domain's home. A configuration file that cannot be read is left alone rather
  than replaced.
- **A deployment that moved its name registry never reached anyone who had
  already visited.** Two settings a domain publishes for routing — which name
  registry to trust, and the ceiling this app puts on how long a name binding
  stays valid — were only re-read when the *publisher's identity* also changed,
  which is an unrelated event. A deployer who added or moved either one reached
  first-time visitors and nobody else, indefinitely. Both are now re-read on
  every load that obtains the domain's configuration. Settings you control —
  how the app opens, the site posture, and a registry you pinned yourself — are
  still yours and are still never overwritten.
- **A withheld app cost nine seconds of waiting before saying so.** The retry
  ladder treated a publisher's 404 the same as a dropped connection, so an answer
  that arrived in the first 200 ms was argued with five times. A 404 or 410 is
  terminal; a 5xx, a timeout or a truncated body is not.
- **"The site is not here" is four different facts**, and both the warm and cold
  surfaces now say which — a withdrawn site, a wrong host, an unreachable origin
  and an origin that faulted previously all produced the same sentence, one of
  them with advice that could not work.
- **A Site Browser bound to any peer but the primary one reported every site as
  unreachable.** The two surfaces that register a site's origin wrote it under
  the system peer while the reader looked under the peer its window was bound
  to; `open "Site Browser" @somepeer` reached this today.
- **One rule now decides which files may be cached forever.** It was written four
  times, no two the same, while the documentation asserted they could not
  disagree. Both directions were wrong in the field: one spelling pinned mutable
  HTML for a year on any site whose source tree is named `content/` (Hugo, Zola
  and Lektor all are), and the other silently dropped long-lived caching from
  every prefixed deployment. The rule is self-verifying — a content-addressed
  path whose directory shards match its own hash — and the two Rust servers, the
  dev server and the published recipe are all held to one shared vector file.

### Fixed — the service worker

- **A rollback poisoned the offline shell.** The worker cached every navigation
  under the canonical `/`, which was correct while `/` was the only navigable
  document. Booting a retained build then overwrote the offline shell with the
  rolled-back one — and that entry outlived all three of the pin's ways out,
  since each of them only runs on a load of `/`, which was being served from it.
  A retained build now caches under its own address, which also makes a pinned
  build work offline for the first time.

### Fixed — connections between devices

- **On a fresh profile, peers could find each other and never connect.** The
  mechanism that lets a browser be dialled back was decided once, at startup,
  from settings a brand-new profile does not have yet — so adding a connector
  during a session had no effect until you reloaded. Discovery and reachability
  are separate mechanisms and only the second was broken, which is why the
  roster lit up with names that could not be reached. A profile that had been
  used before carried the setting and worked, so this read as a regression when
  it was not. Meeting someone and chatting now works without a reload.
- **A connection that died while your machine slept took over two minutes to
  notice.** Everything needed to recover was already built and simply never
  triggered: the app suppresses its own reachability probing for peers it
  believes are connected, and after a suspend that is every peer. Waking is now
  detected from the frame loop — which does not advance while suspended — and a
  single probe turns a ~130-second wait into one round trip.
- **A peer that vanished was never noticed, and its counterpart kept showing it
  as connected.** Roughly half the time the loss surfaced after 30 seconds and
  the rest of the time not at all. It was one defect on two code paths: a link
  carries two handshake roles, and only one had been fixed. Detection is now
  under a second on both. *This fix is in `entity-core-rust`; this application
  needs a build linked against a kernel that carries it.*
- **"You are not on your main peer" was shown for three different causes**, and
  named the rarest one. On the Linux desktop specifically, the WebView ships
  without the WebRTC bindings compiled in, so that window can act as a
  rendezvous point but never as a WebRTC peer — it now says so instead of
  sending you to fix a setting that is not the problem.
- **A desktop install now serves its rendezvous point and app server by
  default.** The whole zero-configuration path — open the app on your phone at
  the desktop's address and you are already paired — existed in full and was
  switched off behind two settings you had to know about. Turning either off
  explicitly is still respected. The setting that reaches the internet rather
  than the local network deliberately did *not* change.
- **The app server moved to a random port when its own was taken**, and the
  thing taking it was often an older copy of this same app — so another device
  got a working UI running stale code, which presented as unrelated bugs. It now
  refuses to start and names the port.

### Fixed — publishers, succession and build identity

- **A deployer can now declare that one publisher replaced another.** A client
  may safely infer this for a domain's home publisher, because that is a single
  slot; it cannot for the others, where a name disappearing as another appears
  is ambiguous between a replacement and one tenant leaving as another arrives.
  Guessing would write a replacement record against a publisher that is alive.
- **A publisher a domain stopped hosting kept a registered address that 404s**
  on every visit. Those are now un-named when the domain stops listing them —
  the name is removed, never the cached content, and the next publish restores
  it. A domain that lists no publishers at all withdraws nothing, deliberately.
- **The update prompt followed the wrong file.** It fired when the service
  worker's bytes changed — an asset touched three times all year — rather than
  when the application itself changed. Measured across two real deployments: the
  one carrying a substantial fix notified nobody, and a worker-only change would
  have notified everybody about an application that had not moved. It now
  compares the application build and stays quiet on a deliberately pinned one.
- **Entity data is now always canonically encoded.** Five encoders built entity
  bodies with a general-purpose encoder that preserves the author's key order,
  so the recorded content hash was over bytes no peer would reproduce — which
  became fatal for every write once the kernel began validating them. Silent,
  because those writes do not wait for a result.
- **A build is identified by a pair, and it can now be pinned.** This
  application links its kernel by path dependency with no cross-repo lockfile,
  so the same commit built twice against different kernel checkouts produces
  different bytes. Both halves are stamped into the shipped page, every verb
  that hands bytes to someone else refuses an unidentifiable tree, and
  `CORE_RUST_REF=<ref>` now *builds* the kernel commit you name rather than
  requiring it to already be checked out.

---

*Everything above is the first theme — a deployment going wrong, and getting
out of it. What follows is the second: writing to the tree rather than only
reading from it.*

### Added — a feed, and posting into your own tree

- **A Feed window.** Follow publishers, read what they posted, and post
  yourself. It opens on the reading pane; *Your feed* and *Manage sources* are
  tabs beside it rather than sections stacked underneath, because with a real
  archive on screen anything below the first pane is a screen and a half down.
- **You do not need a peer id to start.** The window lists *Publishers you can
  reach* — the peers this deployment already knows how to route to — and you
  pick one. Pasting a peer id still works and is the way to reach someone the
  deployment has never heard of; it is the escape hatch, not the front door.
  A publisher you have named yourself is shown under your own name for them.
- **Posts carry dates, and long ones are read whole.** A post too large to
  store inline is published as a pointer to its own content, and the reader
  resolves it — previously such a post arrived showing only its title.
- **Every post says whether it is signed, and by whom.** Each entry is
  verified against a signature the author minted for that post specifically, and
  the result is one of seven distinct answers — *verified*, *unsigned — nobody is
  named for this post*, *could not be checked — this peer id carries no key*,
  *signed by someone other than the author*, and so on. **A post we could not
  check is never shown as one that passed.** Verification needs no second
  fetch and no key exchange, and works with the publisher's origin switched off.
- **A feed can be read two ways, and the window tells you which it used** —
  *Read live, directly from this publisher* or *Read from this publisher's
  published site*. If one route answers with nothing, that is not taken as
  evidence about the other.
- **You can read through someone else's collection.** *Read through* lets you
  name a peer who gathers other people's posts, and read the authors they carry
  — each entry still verified against **its own author's** signature, not the
  gatherer's. The list is typed in rather than discovered, because nothing in
  the protocol yet says where a gatherer is or that one exists.
- **Posting, and what removing means.** *Your feed* writes a post into your own
  tree, which is what publishing is — there is no separate upload step, and the
  post survives a reload. *Remove* is reported honestly as what it is:
  *"Removed from your feed — people who already have it still have it."* It is
  never called deletion, because it is not.
- **A tab running on a temporary identity can still post.** When another tab
  already owns this profile's storage, or the browser is storing nothing at all,
  this tab runs as a real peer holding a key minted for the session. It may
  post, and the post is genuinely its own and signed; it is told once — and can
  put the notice away — that what it writes goes when the tab does. Being
  temporary is not a reason to withhold the control.
- **Publishing a feed from the command line.** `publish --ingest-feed=<dir>`
  publishes authored posts alongside sites and apps under one signed root
  (`make site FEED=<dir>`), and `publish --gather=<peer>@<dir>` republishes
  another publisher's posts under your own namespace, carrying their bytes and
  their signatures unchanged (`GATHER=`).

### Added — three new windows, and a status bar

- **Files** — one home for what is on this device, in five places that differ in
  **who can see them**, stated on each: *My files* and *Working files* and
  *Kept by apps* are private and not listed to other devices; *Offered* says
  *"devices you connect to can see these files and pull them."* Add files from
  the device or drop them on the window, download a whole place as `.zip` or
  `.tar.gz`, and *Show in Entity Tree* for any file. Saves can be downloaded as
  a file and imported on another device.
- **System Monitor** — what this browser tab is actually doing, and what the
  browser refuses to tell it. Time the tab spent frozen in the last second,
  split into drawing this app's windows versus other work on the same thread,
  with a plain reading of what that means for you (*"Smooth: taps and keys are
  answered right away"* / *"Struggling…"*). Per-window download totals, and per
  app the CPU and memory it reports about itself — with apps that report nothing
  named as such rather than counted as zero. Where a figure is unavailable the
  window says which browser capability is missing instead of showing a blank.
- **Storage** now says **what** is using the space, in bytes: content-store
  blobs, live entity data, save-state paths, and files by their place in the
  File Manager — rather than one total.
- **A status bar** across the bottom, with small inline gauges: how much of each
  second this tab was held past one frame (*"the time taps and keys waited"*),
  how much of that was this app drawing its own windows, and the busiest app
  that reports its own work. A healthy profile is a bare mark, not a number.

### Added — apps

- **An app can hand the host a file, and ask for one back.** Opt-in and
  namespaced, so an app that does not use it is unaffected; a file an app hands
  over names the app it came from and can be saved to the device or offered to
  peers.
- **Asset bundles.** An app asks the host for a key and boots from what is in
  the tree, rather than carrying everything in its own bundle.
- **A running app is no longer torn down by another window's download.** A
  foreign catalog or bundle arriving while you were mid-game used to remount
  the player.

### Added — sites, publishers, and finding people

- **A figure opens at full size.** Previously a diagram rendered at whatever
  size fit the column, which on a phone is a corner of it. It is a plain link
  where there is no JavaScript and an overlay where a link cannot do the job.
- **The Registry Browser asks a publisher what they carry** instead of guessing
  and opening a window that may have nothing in it. The answer comes from the
  publisher's own signed tree, so *"they publish none"* is a verified negative
  rather than a third party's claim about somebody else.
- **Find peers here** — one button from an address to a meet, instead of
  composing the steps yourself.
- **Waiting to meet someone is now a duration, not a counter.** The search runs
  for two minutes and says how long is left, at roughly half the network traffic
  the old thirty-second window used.
- **A window keeps the height you give it**, and an app's screen fits the window
  it is in.
- **If the app itself dies, the page says so.** A failure that stops the
  application from running is now reported by a layer outside it, with a
  Reload and a link to System Recovery — previously every detector we had lived
  inside the thing that had stopped.

### Changed in ways that can break an existing caller

Two: one on the publisher command line, one in the published tree. Nothing in the
deployment document, the authored source layouts or a stored profile moved — a
`0.9.0` deployment config and a `0.9.0` `--ingest` directory are read unchanged,
and a profile from `0.9.0` comes back without a migration.

- ⚠️ **`publish` no longer assumes what to publish when you do not say.** It now
  requires exactly one of `--ingest=<dir>`, `--demo-sites` or `--no-sites`
  (`INGEST=` / `DEMO_SITES=1` / `NO_SITES=1` through `make`), and refuses
  silence. **This is a breaking change to the publisher command line:** an
  invocation that worked at `0.9.0` will now exit non-zero and tell you which
  three words to choose between. Silence used to mean `--demo-sites`, and
  because the publish is wholesale, a command that meant `--ingest` and omitted
  it replaced a domain's real sites with the bundled demo set, under that
  domain's own identity, and exited `0`.
- ⚠️ **A site asset over 16 KiB is published as a pointer, and a `0.9.0` reader
  cannot follow one.** The convention caps an inlined payload at 16 KiB, so
  anything larger is now stored as a content-addressed pointer with its blob and
  chunks in the published closure. **This release reads both**, so a tree
  published at `0.9.0` still resolves. The other direction does not: a tree
  published at `0.10.0` and read by a `0.9.0` build shows nothing where those
  assets were, which on a real published site was **629 of 1,346 assets — most of
  the figures**. It matters if something other than this app reads your published
  trees, or if a visitor is pinned to a retained `0.9.0` shell; an ordinary
  visitor is carried to the new build on the next load and never sees it.

### Known limitations

New in this release, or newly stated:

- **A post you can read is not necessarily a post you can find.** The feed lists
  publishers this deployment already routes to, and lets you paste a peer id for
  anyone else. There is no mechanism for learning that a publisher exists whom
  you have no route to — that is a protocol question nobody has answered yet,
  and the window does not paper over it by guessing.
- **Nothing in the app publishes a gathered feed.** You can read through a
  gatherer, and `publish --gather` builds one from the command line, but there
  is no in-app surface for gathering — the capability is complete and verified
  end to end, and a person cannot reach it.
- **A long post shows only its title in two places.** Resolved for a feed read
  from a publisher's own site; a post read through a gatherer, and one of your
  own, still show the title, and say so rather than appearing empty.
- **Permission enforcement is off.** A peer you are connected to can read any
  path in your tree. Sharing is expressed and recorded, and nothing refuses a
  read yet — so treat a connection as full read access to that profile.
- **Your own files live under an application-scoped path.** The address is
  non-conformant with where a person's own content is supposed to sit; moving a
  durable user-content root with no export path is the destructive direction, so
  it moves together with the work that reworks how private and shared files are
  distinguished.
- **There is still no export path.** Nothing in the app can hand you your whole
  tree, which is why every recovery action here is scoped to app code and
  refuses to touch your data.

- **Rollback is partial.** A retained shell runs against the **current** service
  worker and the current `sw.js` — those are unhashed and the origin serves
  exactly one of each. So if the defect you are rolling back from lives in the
  worker or in `sw.js`, rolling back the shell does not escape it. The converse
  also holds: a change confined to unhashed assets does not produce a new build
  id, and so is not a distinct rollback slot.
- **Content is stored once per hosting prefix, not once per domain.** Two
  publishers sharing an origin do not share the content store, so byte-identical
  blobs are uploaded and billed under each prefix — measured at 17 of 19 blobs
  duplicated for two peers publishing the same site set. Content addressing makes
  sharing safe (a hash is a self-certifying name); the store simply is not global
  yet.
- **Recovery from a publisher changing identity covers the domain's home
  publisher only.** A deployment may host several publishers at one origin. If
  one of the others changes identity, nothing detects it and stored references to
  it are not repaired — a deployment configuration can state *who* publishes but
  has no way to say that one publisher *replaced* another, and guessing from a
  peer appearing as another disappears would be wrong as often as right. No
  deployment we publish hosts more than one publisher today, so this is a gap in
  what the mechanism covers rather than a fault you can currently meet.
- **Nothing sets a durable build pin yet.** `?build=<id>` is real for an operator
  or for support walking someone through a recovery, and inert for everyone else;
  no crash-loop detection arms it automatically.
- **The desktop app has rollback exposure without rollback machinery.** Its data
  directory carries no version, and reinstalling the previous installer is
  ordinary behaviour when someone hits a bug — so an older build can read data a
  newer one wrote, with no schema floor and no version check. There is no
  updater to carry anyone forward, which makes the exposure less likely to be met
  and open-ended once it is.
- **The desktop app's LAN publishing has no TLS**, so a phone loading the app
  from `http://<desktop-ip>` is on an insecure origin and loses three things
  uniformly: service workers (so no offline shell — that browser cannot open the
  app while the desktop sleeps, even though its data is on the device), OPFS
  (which degrades to IndexedDB and is not user-visible), and camera access — so
  the QR scanner, which exists to save you retyping a peer id, is unavailable on
  precisely the origin that forces the retyping. A self-signed certificate does
  not fix this: Chrome refuses to register a service worker on a certificate-error
  origin even after you click through, while Firefox honours a manual exception,
  so it would work on one engine and not the other.
- **Automatic checking cannot confirm the 17 Latin-script translations.** Correct
  German often looks like English, so no mechanical signal separates a genuine
  translation from a skipped one; the gate covers the 13 non-Latin locales and
  says so in its own output. Those 17 were translated in the same pass and read
  by eye. If you find a string in your language that is wrong or still in
  English, that is worth reporting — it is the one part of the localization we
  cannot prove.

## [0.9.0] — 2026-08-24

The first release since `v0.8.0` (2026-06-21), and a large one: **two peers on
two machines can now find each other, connect, chat, and send each other
files** — none of which the previous release could do. Still a **research
preview**: suitable for evaluation and exploration, not a hardened production
deployment.

### Added — peer-to-peer connectivity

- **WebRTC transport between two browsers.** Two browser peers on different
  networks establish a data channel through a signaling node and talk directly.
  Proven across **two separate NATs** with a reflector, and with a negative
  control (no reflectors ⇒ no media) so "configured" and "works" are not
  confused.
- **Meet at a name.** You add a signaling node once, then `meet tag <label>` —
  both sides learn each other's peer id and connect. No copying peer ids by hand.
- **Rendezvous nodes** — a durable registry of signaling nodes on the system
  peer, each carrying its own STUN reflectors. A node can also **advertise**
  what it serves, so a node learns its reflectors without anyone typing one.
- **A TURN relay can be configured**, in its own three fields (URL, username,
  credential) beside the reflectors. The two are kept apart on purpose: a
  reflector is a commodity and is credential-free by spec, a relay is rented and
  credentialed, and merging them would attach a username to entries that must
  not have one. A relay URL with **half** its credentials is refused where you
  typed it — `RTCPeerConnection` accepts one, shows it as configured everywhere,
  and gathers no relay candidates at all. The credential is stored in this
  peer's tree in plaintext, like the rest of the app's configuration; it is not
  a secret store. See *Known limitations* — configuring a relay is proven,
  relayed media flowing through one is not.
- **Connection state is now the kernel's, and it tells the truth.** Every surface
  showing whether a peer is connected subscribes the kernel's own status rather
  than guessing from connection attempts — the previous release could report
  "Connected" straight through a dropped link.
- **Automatic reconnect**, driven by the network extension rather than a
  hand-rolled loop, per conversation as well as for the desktop backend.
- **A dialed address becomes a route** the kernel owns and remembers, so a
  reconnect after a reload does not need the app to hand the address back.

### Added — file transfer

- **Send a file from one browser to another.** Offer a file from the File
  Transfer window; the other side sees it and pulls it. Content-addressed and
  chunked, so it rides whatever connection reaches the peer — a WebRTC data
  channel or a WebSocket, with nothing in the transfer naming either.
- **"Stop offering"** withdraws the listing from the far side.
- **Desktop share.** The Tauri app mounts a share directory
  (`~/.entity/tori-share`) so a browser can browse it, pull from it, and upload
  into it.

### Added — chat

- **1:1 and N-party conversations**, delivered over the same peer connection —
  including over WebRTC between two browsers. Authorship is derived from path
  authority rather than claimed in the message.

### Added — apps

- **L5 app hosting** — an app can now be a full WASM entity-peer running a
  compute program behind the iframe boundary, not just opaque JS UI. Includes a
  program chrome bar, an on-screen input source, a fixed-rate tick loop, and a
  debug overlay with a live tree dump and single-step. Program fixtures are
  verified **hash-identical against the Go oracle**.
- **Full screen for a running app** — `⛶ Full screen` in the player bar takes the
  app to the whole display. The two size controls now say which is which: the
  window header's `▢` makes the *window* fill the viewport, `⤢ Expand` makes the
  *stage* fill the window. In full screen one thin bar remains, deliberately —
  it is the discoverable way back out.
- **An app can keep the screen from blanking.** A long-running app (an AI turn,
  a simulation) can hold a screen wake lock, which the browser denies inside a
  frame unless the embedder delegates it. Granted to every app: the platform
  already bounds it — the lock is released automatically the moment the app's
  document is hidden, so a backgrounded app cannot hold your display on. Needs a
  **secure origin** (localhost or https); on a plain-http LAN address the browser
  API is absent entirely.
- **Save-state management** — back up, restore, and hand an app save to another
  peer. The transfer is an ordinary file offer, pulled rather than pushed, and an
  incoming save backs up whatever it replaces before landing.

### Added — the app in 30 languages

- **Full interface localization: 30 locales × 701 keys**, with lint gates that
  fail the build on an untranslated surface, on a missing key, and on prose that
  never made it into a catalog. A missing key ships the raw key rather than
  silently falling back to English.

### Added — visibility and trust

- **Access Log** — a user-facing capability audit: what was dispatched, in which
  direction, by which peer, with an aggregated observed-capability map.
- **System Overview** — one window for the system peer(s), the desktop backend's
  live log stream (with level control), and its storage.
- **Sharing (foundation).** A share is expressed as an ordinary capability grant
  rather than a second permission system, and offering a file publishes one.
  See *Known limitations* — the enforcement half is not finished.

### Added — publishing and deployment

- **Durable publisher identity** is the default, so a republished site keeps its
  address instead of moving each run.
- **First-paint site discovery** — published sites appear on the first render
  rather than after a navigation.
- **Static export emits assets and rewrites image sources**, so a published
  static site's images resolve.
- **`entity-deployment.json`** shapes one generic bundle per domain, and
  `make site-serve` emits it by default.
- **`make tauri-bundle`** bakes published sites and apps into the desktop binary,
  served offline same-origin.
- **A registry pin can be set at publish time** —
  `make site-dist DEPLOY_CONFIG=1 REGISTRY_PIN=PEER_ID@ORIGIN` seeds it into
  `entity-deployment.json`, so a deployment ships knowing which name registry to
  trust. It refuses a pin a visitor's browser could never use — a malformed
  peer-id, or an origin with no peer-id at all — on the machine where you can
  still read the error, rather than at a visitor's browser as *"this registry is
  down"*. Previously the only route was hand-editing the emitted JSON, which
  skips exactly that check.
- **Emitted trees name the commit that built them** — published `index.html`
  carries `<meta name="entity-build">`, so a deployed site can be traced back to
  a build without guessing.
- **A link to a site outside the published set now refuses the publish.** A
  `site:` or `entity://` link whose target is not in this export resolves against
  the current domain, so under per-domain publishing it shipped as a 404 that
  read like a withholding origin. The publish audits page bodies *and* generated
  navigation, names every offender, and exits non-zero.
  `ALLOW_OUT_OF_SET_LINKS=1` is the escape hatch for a deliberately partial set
  mid-migration.
- **The publish plan accounts for apps, not just sites.** Re-publishing without
  `--ingest-apps` deletes the apps tree; `--plan` reported *"nothing would be
  removed"* while that happened, and the ordinary publish now warns too — so
  forgetting a flag can no longer silently empty a live domain's app catalogue.
- **Publish output cannot be written outside the repo by accident.** A
  containerized publish with an out-of-mount destination used to print success
  and write nothing to the host. It refuses now, on the machine where the error
  can be read.

### Added — the release build

- **`make dist` — the release build.** Produces the OS-native installers for the
  host you build on (Linux `.deb`/`.rpm`/`.AppImage`, macOS `.dmg`, Windows
  `.msi`/NSIS `.exe`) into `artifacts/`, under the fleet-wide filename scheme
  `{name}_{version}_{os}_{arch}`. Previously the `tauri*` targets produced only a
  debug test binary, and packaging was unwired.
- **`make dist-web`** — the browser SPA as a release tarball.
- **`make dist-native` / `NATIVE=1`** — the same recipes on the host toolchain
  (ADR-0019's `-native` opt-in), for building on a machine whose toolchain you
  manage yourself.
- **Windows installers can be cross-built from Linux** —
  `make dist DIST_OS=windows XWIN_ACCEPT_LICENSE=1` produces a real NSIS
  `_setup.exe` with nothing but make + podman. The toolchain image gained
  clang/lld/NSIS + cargo-xwin. (`.msi` still needs a Windows host — WiX is
  gated on it, and a `.msi` asked for from Linux is silently omitted rather
  than refused. macOS cannot be cross-built at all: Tauri's macOS bundlers are
  `#[cfg(target_os = "macos")]`.)
- **Tag-triggered release workflow** (`.github/workflows/release.yml`) — five
  platforms plus the web bundle, `checksums.txt`, and a cosign keyless signature
  over it, published as a GitHub Release. `workflow_dispatch` runs the whole
  thing as a dry run that publishes nothing.
- **A version-coherence guard** — `make dist` fails if `Cargo.toml`,
  `src-tauri/Cargo.toml` and `tauri.conf.json` disagree about the version.
- **Build provenance** on every artifact (`actions/attest-build-provenance`) —
  verify with `gh attestation verify <file> --repo EntityChurch/entity-browser-rust`.
- **Permanent download links** — each artifact is also published without the
  version in its name, so `/releases/latest/download/entity-browser_linux_x64.deb`
  never goes stale.
- **`update.json`** — a version manifest published with each release so an app
  can *tell* the user a newer version exists. Deliberately carries no signatures
  and installs nothing: there is no update-signing key, so no one can push code
  to installed machines. The in-app check is a later, default-off setting.

### Fixed

- **A running app restarted itself every few seconds.** Playing anything that
  saves as you go — reported as *"it keeps knocking me back to the start
  screen"* — reloaded the app roughly a second after each move: the Apps window
  watched the save area so its Saves panel could read it, and the running app's
  own save then triggered a rebuild that remounted it. Present in every
  deployment published since saves shipped.
- **A published site's navigation ran off the top of the page.** A site whose
  nav has groups — which the real corpus does, one of them 23 links — flattened
  every level onto a single row, reported as *"the menu items just shoot off
  like every item across the top."* The markup was always a proper tree; the
  stylesheet laid out nested lists as if they were siblings. Groups are now
  collapsible dropdowns with **no JavaScript**, closed by default with the group
  holding the current page marked, opening as a multi-column panel anchored to
  the header so it cannot open off-screen at any window width. Static pages
  only; the in-app site browser was never affected.
- **Every link the Share button produced was unopenable.** *Share link* emitted
  a `self` marker meaning *"the peer of whoever is reading this"* — correct when
  it was written, because a same-origin export could not know the live peer's id
  at publish time, and wrong once sites were published under a durable publisher
  identity. So a link copied out of any published domain asked the recipient's
  own browser for a site it had never held, and opened nothing. The tell was in
  the error itself: it named a **different peer for every visitor**. Shared
  links now carry the publisher's peer id, and the static exporter already did.
- **A site published by someone else read as deleted.** Following a shared link
  to a publisher on another domain reported *"No site manifest"* — a confident
  *that does not exist* — about a site that was live and one domain away. The
  app was not merely missing a route: opening such a link registered the
  **current** domain as that publisher's host, so it looked for their content on
  the wrong server, got a 404, and reported the site missing rather than the
  lookup wrong. The message now names the address it actually asked and says the
  site belongs to another peer, so a wrong lookup reads as one. The underlying
  guess is unchanged in this release; only the report of it is.
- **Resolving a name in the Registry Browser led nowhere.** Looking up a name
  against a pinned registry worked — it walks that registry's signed root and
  reports what it checked — but *Open in Site Browser* did nothing at all, and
  once it did open a window that window was empty, reporting *"No external sites
  cached"* about a publisher whose sites had just been fetched into local
  storage. Two causes: the button never raised the action that opens a window,
  and the window it eventually opened was pointed at the publisher's storage
  rather than your own, where a visited publisher's content actually lives.
  Resolving a name now opens a browser listing that publisher's sites, and they
  can be read. As everywhere else on this path, each is labelled **not
  verified** — the pages are fetched from the origin the registry named, and the
  origin still chooses what each address returns.
- **A Chat window could be pointed at one person and never at anyone else.** The
  picker disappeared once a conversation started, so the only way to talk to a
  second person was to open another Chat window. There is now a way out of a
  bound conversation; leaving does not disconnect the peer or delete the
  history, and re-entering shows it again.
- **Refresh in File Transfer made the file you were looking at disappear.**
  Pressing it cleared the listing before re-asking, so against a peer that
  shares no folder — which is every browser peer — the rows never came back and
  the pane sat on a spinner. Refresh now keeps showing what it has while it
  re-asks, and a file removed on the far side actually leaves the list.
- **Every page load re-downloaded the 17 MB worker bundle, once per worker.**
  `entity-worker_bg.wasm` carries no content hash in its filename, so the
  service worker treated it as mutable and fetched it with `cache: 'reload'` —
  which deliberately bypasses the browser's HTTP cache. A visitor paid it on
  every load, forever, and no CDN or cache header could help. It is now served
  from the service-worker cache and re-fetched only when the build changes,
  keyed on the main bundle's hash so the two halves of the runtime can never
  disagree. Measured: **3 downloads across 3 loads → 1**.
- **The app had no `.ico` or `.icns`, so it could not be built for Windows at
  all** — `tauri.conf.json` listed a single 32×32 `icon.png`, and `tauri-build`
  fails outright with *"icons/icon.ico not found"*. The full icon set is now
  generated and wired into the bundle config. (The source art is still a
  placeholder upscale — replace `src-tauri/icons/icon.png` with real 1024×1024
  art and re-run `cargo tauri icon`.)

### Changed

- **`ENTITY_PROFILE` is gone.** The opaque `full` / `tutorial` / `strict-site`
  build presets are replaced by `ENTITY_STARTUP_SURFACE` (`chrome` / `site` /
  `window`, plus `ENTITY_STARTUP_WINDOW_TYPE`), which bakes only the cold-boot
  surface; the granular posture — site mode, whether visitors may create peers, a
  locked kiosk — is a per-domain `entity-deployment.json` concern rather than
  something compiled in. Setting `ENTITY_PROFILE` now does nothing.
- **`make publish` → `make site`** (also `publish-bare` → `site-bare`,
  `publish-serve` → `site-serve`). `publish` is a reserved verb fleet-wide,
  meaning "push a package to its language's native registry" (ADR-0023
  Amendment 1). The old names currently fail with a pointer to the new ones and
  will be removed after one release. **The app's own CLI is unaffected** —
  `entity-browser publish <dir>` is unchanged.
- The toolchain image resolves its pinned binaryen by architecture instead of
  hardcoding `x86_64`, so it can build on an aarch64 host.
- **Startup posture is two real axes, not presets.** The opaque
  `full`/`tutorial`/`strict-site` profiles are gone; a deployment sets `surface`
  (`chrome` / `site` / `window`) directly, plus granular site-mode and
  peer-creation settings. **If you deploy this over an older one, re-emit
  `entity-deployment.json`** — a config naming a `profile` is now ignored.
- **Peer vocabulary is honest about role, runtime and storage** — the old
  frontend/backend split said less than it implied.
- **Every `make` target runs in the container.** `tauri-run`, `serve`,
  `e2e-worker` and the publish targets no longer need host tools; the host needs
  only `make` and `podman`. `make host-run` is the opt-out for running the
  built binary natively.
- **`Cargo.lock` is now committed** (both of them). This repo ships binaries, and
  a release build has to resolve the same dependency versions twice.

### Known limitations

Stated rather than discovered after install:

- **Sharing is open-posture.** Grants are not enforced by default, which is why
  browser-to-browser transfer works out of the box. Offering a file *does*
  publish a real grant, scoped to exactly the two reads a puller performs and
  read-only by construction — but with enforcement off those grants are inert,
  and no deployment has been run with it on. There is also no *"who may pull
  from me"* surface, so an offer is readable by anyone who can reach you and
  there is no way to choose a narrower audience.
- **An offer is permanent.** Withdrawing an offer removes the *name*, not the
  bytes: the content store has no forget operation, so nothing reclaims what was
  ingested. "Stop offering" is worded that way deliberately and never says
  "delete".
- **One offered file is capped at 16 MiB** — an offer-side memory bound, not a
  protocol limit.
- **Choosing a file does not work in Firefox for Android.** Tapping *Offer a
  file* asks the browser for a file chooser; Firefox accepts the request and
  then closes the chooser itself, in about 200 ms, without ever showing it.
  Measured on one device across all nine ways a page is allowed to open a
  chooser — hidden, rendered-but-invisible, plainly visible and tapped directly,
  script-opened, `showPicker()`-opened and `<label>`-activated. Every one is
  dismissed the same way, and **Chrome on the same phone, same page, same file
  works normally**, so this is the browser rather than the app or the device,
  and there is no setting or markup on our side that changes it. The app now says so instead
  of appearing to do nothing. Everything else in File Transfer — browsing,
  pulling, receiving — is unaffected; it is only picking a file to send.
- **On the Linux desktop app, other devices cannot reach *this* window's peer.**
  The Linux desktop WebView (WebKitGTK) ships without `RTCPeerConnection` at
  all — measured on two distributions, on a secure origin, with the engine's own
  WebRTC switch turned on. WebRTC is how a peer that has **no address** gets
  reached, so what this costs is precisely that: after meeting someone by name,
  the connection back to the desktop's own Chat and File Transfer windows never
  establishes. The app says so on boot rather than appearing to hang.
  **What is unaffected**, because it never used WebRTC: connecting *by address*,
  the desktop's shared folder and anything served over that connection, and
  browser-to-browser chat and file transfer that merely *rendezvous* through a
  desktop running the signaling node — the common case, and the one in the
  two-device transcript. **Windows and macOS use different engines and have not
  been measured**; they are not known to have this, and are not known not to.
- **Symmetric NAT is untested.** Two peers behind symmetric NATs generally need
  a TURN relay, which this release can be *configured* with — but no gate points
  it at a real TURN server, so what is proven is that the configuration reaches
  the ICE agent, not that relayed media flows. Nothing here diagnoses a relay
  that fails to allocate, either: it looks like "this network needs a relay" to
  someone who already has one.
- **App save-state keeps one *live* save per app.** Backups are timestamped and
  restorable, so an earlier state is recoverable — but there are no named slots,
  and an app cannot hold two independent games at once.
- **Content authored in the Site Editor cannot be published** to a static site —
  the publish pipeline reads from disk, not from a running peer's tree.
- **macOS artifacts and the Windows `.msi` have not been executed** on their
  native runners; the Linux artifacts and the Windows NSIS installer have.
- **Installers are unsigned** on Windows and macOS, so both will warn on first
  run. Linux packages install normally.

## [0.8.0] — 2026-06-21

Initial public research-preview release. Predates this changelog; see the
GitHub release notes for that tag.

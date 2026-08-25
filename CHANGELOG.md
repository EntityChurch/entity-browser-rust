# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A **tag is a release** — a push to `master` that changes nothing you would
version-pin against doesn't get one (ADR-0015). Each release's notes on GitHub
are generated from that version's section below, so write it for the person
downloading the artifact.

## [Unreleased]

_Nothing yet._

## [0.9.0] — unreleased (runway)

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

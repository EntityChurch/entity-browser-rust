//! Serve the SPA from this desktop, so another device on the same network can
//! reach it by URL.
//!
//! # Why this exists
//!
//! Tori already *has* the app: `frontendDist` is embedded in the executable and
//! the WebView loads it from there. What it did not have was a way to hand that
//! same bundle to the phone sitting next to it, so the two-machine flow needed
//! **two commands** — `make tauri-run` for the desktop and `make pair-serve` (a
//! whole second release build) to put the SPA somewhere a browser could fetch
//! it. That is one command too many for the most common thing anyone will do
//! with this product.
//!
//! # The part that matters more than the file serving
//!
//! Joining a rendezvous means getting a Base58 peer-id onto another device, and
//! every channel we have is bad: retyping it is where a real two-machine run
//! stalls (a typo presents as a *connectivity* failure), and the QR scanner
//! needs `getUserMedia`, which an insecure origin denies — so the affordance
//! built to avoid retyping is dead on exactly the origin that forces it.
//!
//! `make pair-serve` answered that by **baking** the node in at build time.
//! This answers it without a build: the process serving the app is the same
//! process running the rendezvous, so it already knows its own node peer-id and
//! listen address, and it puts them in the URL it redirects to. A browser that
//! merely loads `http://<this-box>:8081` is provisioned before boot — no copy,
//! no QR, no `connector add`, and **no reload**, because the node is present
//! before the app starts rather than added after it [AP22's positive form].
//!
//! The redirect target is the `?webrtc_node_peer=…&webrtc_node=…` pair, which is
//! the **top** of `connectors::resolve_provisioning_quietly`'s precedence chain
//! and is already the channel the WebRTC e2e harness uses. Nobody types it; the
//! typed URL is the bare host.
//!
//! # Deliberately not a dependency
//!
//! HTTP/1.1 for `GET`/`HEAD` over a fixed in-memory asset map is a request line,
//! a header block and a response — smaller than the code that would configure a
//! web framework, and it is entirely testable against a plain `TcpStream`. Same
//! call this repo already made for PCP/NAT-PMP in `port_mapping.rs`.
//!
//! # What this is NOT
//!
//! Not a public web server. It binds the LAN, speaks no TLS, serves a fixed
//! asset map and nothing from the filesystem, and refuses every method but
//! `GET`/`HEAD`. Off unless the user turns it on.
//!
//! # Why no TLS, and what it would actually take (asked 2026-09-03)
//!
//! Everything this origin loses is gated on being a SECURE CONTEXT, whose only
//! plain-HTTP exceptions are `localhost` / `127.0.0.1` — which is this desktop,
//! never the phone. Three losses, and they are not equal:
//! **the service worker** (so no offline shell: the phone cannot open the app
//! when this machine sleeps), **OPFS** (degrades gracefully — the Worker arm
//! falls back to Direct/IndexedDB, which works fine here), and
//! **`getUserMedia`** — the sharp one, because the QR scanner exists precisely
//! to stop people retyping a Base58 peer-id and it is dead on exactly the
//! origin that forces the retyping.
//!
//! **A self-signed cert does not fix this, and is worse than not trying.**
//! Chrome refuses to register a service worker on a cert-error origin *even
//! after the user clicks through the interstitial* (`SecurityError: Failed to
//! register a ServiceWorker: An SSL certificate error occurred when fetching
//! the script`); Firefox honours a manually-added exception. So it would work
//! on one engine and not the other, which reads to a user as a browser bug.
//! `tools/dev-cert.sh` is where that was measured.
//!
//! **What works is a certificate the DEVICE already trusts, and there are only
//! two shapes of that.** (1) Install a private CA on the phone — which is what
//! `dev-cert.sh` mints for development, and its own header says never install
//! it on a device you care about. Not shippable. (2) A publicly-trusted cert
//! for a real DNS name whose A record points at the **private** IP — the
//! `plex.direct` model, and the thing routers do with `routerlogin.net`. That
//! genuinely works: valid cert, valid name, real secure context, no
//! interstitial.
//!
//! **The reason (2) is not simply "the answer" is that it inverts the
//! product.** It requires owning a domain and running DNS, and the leaf's
//! private key would ship inside every install — so it is effectively public,
//! and a CA is obliged to revoke a knowingly-disclosed key. Per-install ACME
//! (DNS-01) avoids the shared key but then **two machines on the same LAN
//! cannot pair unless our infrastructure is reachable**, for a product whose
//! premise is that they do not need us. That is an architectural trade to be
//! decided, not a TODO to be closed — and it is why this is documented here
//! rather than filed as a defect.
//!
//! Note what already softens the sharp loss: the redirect above provisions the
//! node automatically, so the common path types nothing, and pasting the
//! pairing line covers the camera's job without a secure context.

use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{AppHandle, Runtime};

/// The port we ask for. Matches `make serve` / `make pair-serve` so the URL a
/// person has already seen in this project is the URL that works here.
pub const DEFAULT_PORT: u16 = 8081;

/// A running SPA server. Dropping the handle stops the accept loop.
pub struct AppServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl AppServer {
    /// The address to hand a person, with `0.0.0.0` resolved to this box's LAN
    /// address — a wildcard bind is not something anyone can type into a phone.
    pub fn url(&self) -> String {
        format!("http://{}", crate::connectable_host(self.addr))
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The node a freshly-loaded browser should be provisioned with: this desktop's
/// own rendezvous, when it is serving one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeHint {
    pub peer_id: String,
    /// The `ws://` address a *remote* browser can reach — already through
    /// `connectable_addr`, so never `0.0.0.0`.
    pub ws_addr: String,
}

/// Compose the location a bare `/` redirects to.
///
/// **Both halves or neither.** `resolve_webrtc_provisioning` yields `None` for a
/// half config, so emitting one would install no establisher while looking
/// configured — the silent shape the whole mechanism exists to remove. With no
/// node we serve the app unparameterized: it still works, the user just has to
/// add a connector by hand, which is the pre-existing situation and not a
/// regression.
pub fn redirect_target(node: Option<&NodeHint>) -> Option<String> {
    let n = node?;
    if n.peer_id.trim().is_empty() || n.ws_addr.trim().is_empty() {
        return None;
    }
    Some(format!(
        "/?webrtc_node_peer={}&webrtc_node={}",
        urlencode(n.peer_id.trim()),
        urlencode(n.ws_addr.trim()),
    ))
}

/// Percent-encode everything outside the unreserved set.
///
/// A peer-id is Base58 and needs none of this; a `ws://host:port` address needs
/// `:` and `/` encoded or the query value is truncated at the first one by some
/// parsers. Encoding both through one function rather than trusting the shape of
/// either.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// `Cache-Control` for one asset path.
//
// **Opt in to immutable, never default to it.** The inverse rule — enumerate the
// mutable files and let everything else be `immutable` for a year — is what put a
// one-year cache on the file carrying the registry pin (`tools/cors-serve.py`,
// fixed 2026-08-20); every artifact added afterwards inherits the unsafe value
// silently, and it is invisible locally because a fresh browser has no cache.
// Note also that "no `Cache-Control`" is not "uncached": a browser then applies
// heuristic freshness off `Last-Modified` and serves a stale shell without
// revalidating.
//
// **C15 — THE cache-immutability rule, included verbatim, not re-expressed.**
//
// This file used to carry its own `starts_with("/content/")` +
// `is_hash_named_bundle()` pair. `REVIEW-2026-08-25` §2.1 found four such pairs
// across the repo disagreeing in four different ways, each individually
// reasonable, with `GOTCHAS.md` asserting they could not disagree. The rule now
// lives in ONE file and every Rust call site takes it by `include!` — there is
// no `[lib]` target in the app crate to depend on, and a fifth careful copy is
// the thing being fixed. Both halves of the tree are pinned to the same
// `tools/cache-policy-vectors.txt`; see the test at the bottom of this file.
//
// The `#[path]` module brings in `is_immutable`/`cache_control` and their
// helpers. If it fails to resolve, the app crate moved — fix the path, do NOT
// inline a copy.
#[path = "../../src/cache_policy_rule.rs"]
mod cache_policy_rule;
use cache_policy_rule::cache_control;

/// A parsed `GET`/`HEAD` request line.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: Method,
    /// The target with any query or fragment stripped — the asset map is keyed
    /// on paths, so `/index.html?x=1` is a request for `/index.html`.
    path: String,
    /// **Whether the target carried a query at all**, which the stripped path
    /// cannot tell you and which decides the redirect.
    ///
    /// Load-bearing: without it the redirect target `/?webrtc_node_peer=…`
    /// strips back to `/` and is redirected again — an infinite loop in which
    /// the page never loads. Caught by
    /// `the_server_answers_a_real_request_over_a_real_socket` on its first run;
    /// no pure test of `redirect_target` could have seen it, because each hop
    /// is individually correct.
    had_query: bool,
}

fn parse_request_line(line: &str) -> Result<Request, &'static str> {
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or("empty request line")?;
    let target = parts.next().ok_or("no request target")?;
    let method = match method {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        _ => return Err("method not allowed"),
    };
    if !target.starts_with('/') {
        return Err("request target must be origin-form");
    }
    let path = target.split(['?', '#']).next().unwrap_or("/");
    Ok(Request {
        method,
        path: path.to_string(),
        had_query: target.contains('?'),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Method {
    Get,
    Head,
}

/// Everything about answering one request except the socket, so it is testable
/// without one.
enum Reply {
    Redirect(String),
    Asset { bytes: Vec<u8>, mime: String, cache: &'static str },
    Status(u16, &'static str),
}

/// Where response bodies come from: a path to `(bytes, mime)`.
///
/// A trait object rather than the `AppHandle` directly, so the whole HTTP path
/// — including a real socket round-trip — is testable without standing up a
/// Tauri app. Every pure helper here was already unit-testable; what that could
/// never show is whether the server *answers*, which is the shape of green gate
/// this codebase keeps having to correct for.
pub trait Assets: Send + Sync {
    fn get(&self, path: &str) -> Option<(Vec<u8>, String)>;
}

/// The production source: Tauri's embedded `frontendDist`.
struct EmbeddedAssets<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Assets for EmbeddedAssets<R> {
    fn get(&self, path: &str) -> Option<(Vec<u8>, String)> {
        let asset = self.0.asset_resolver().get(path.to_string())?;
        Some((asset.bytes, asset.mime_type))
    }
}

fn route(assets: &dyn Assets, req: &Request, node: Option<&NodeHint>) -> Reply {
    // A **bare** `/` is the only thing anyone types, so it is the only place the
    // node hint can be attached without the user seeing it.
    //
    // `!had_query` is what terminates the redirect: the target we send them to
    // is `/?webrtc_node_peer=…`, whose path is also `/`. It also means an
    // explicitly parameterized URL is left alone — `?webrtc_enable=0` is the
    // kill switch for isolating whether a failure is WebRTC's, and a redirect
    // that dropped it would defeat exactly the person trying to use it.
    if req.path == "/" && !req.had_query {
        if let Some(target) = redirect_target(node) {
            return Reply::Redirect(target);
        }
    }
    let lookup = if req.path == "/" { "/index.html" } else { req.path.as_str() };
    match assets.get(lookup) {
        Some((bytes, mime)) => Reply::Asset {
            bytes: if req.method == Method::Head { Vec::new() } else { bytes },
            mime,
            cache: cache_control(lookup),
        },
        None => Reply::Status(404, "Not Found"),
    }
}

fn serialize(reply: &Reply) -> Vec<u8> {
    let mut out = Vec::new();
    match reply {
        Reply::Redirect(target) => {
            // 302, not 301: the node changes when the user toggles the
            // rendezvous, and a permanent redirect would be cached past that.
            out.extend_from_slice(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {target}\r\n\
                     Cache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            );
        }
        Reply::Asset { bytes, mime, cache } => {
            out.extend_from_slice(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\
                     Cache-Control: {cache}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(bytes);
        }
        Reply::Status(code, text) => {
            out.extend_from_slice(
                format!(
                    "HTTP/1.1 {code} {text}\r\nContent-Type: text/plain\r\n\
                     Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                )
                .as_bytes(),
            );
        }
    }
    out
}

/// Read a request line, answer it, close. HTTP/1.1 with `Connection: close`, so
/// there is no keep-alive state machine to get wrong.
///
/// The read is bounded and the socket carries timeouts: this listens on the LAN,
/// and a peer that opens a connection and says nothing must not hold a thread.
fn serve_one(mut sock: std::net::TcpStream, assets: &dyn Assets, node: Option<&NodeHint>) {
    use std::io::{BufRead, BufReader, Write};

    let t = std::time::Duration::from_secs(15);
    let _ = sock.set_read_timeout(Some(t));
    let _ = sock.set_write_timeout(Some(t));

    let Ok(peek) = sock.try_clone() else { return };
    // `take` bounds the request line: an unbounded `read_line` on a hostile
    // socket is an unbounded allocation.
    let mut reader = BufReader::new(std::io::Read::take(peek, 8 * 1024));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }

    let reply = match parse_request_line(line.trim_end()) {
        Ok(req) => route(assets, &req, node),
        Err("method not allowed") => Reply::Status(405, "Method Not Allowed"),
        Err(_) => Reply::Status(400, "Bad Request"),
    };
    let _ = sock.write_all(&serialize(&reply));
    let _ = sock.flush();
}

/// Bind and start serving. `node_of` is consulted **per request**, not captured,
/// so toggling the rendezvous changes what a new visitor is provisioned with
/// without restarting the server.
///
/// **Fails when the requested port is taken. It does NOT move (2026-09-07).**
///
/// It used to fall back to an ephemeral port, on the reasoning that *"a
/// developer box routinely has `make serve` already holding 8081, and refusing
/// to start is a worse answer than starting somewhere the caller can read
/// back."* Both halves of that were wrong, and it cost two sessions:
///
/// **The caller cannot read it back.** This server's entire purpose is that a
/// person walks to *another device* and types the URL. They are not looking at
/// this machine's System Overview row when they do it; they type the port they
/// have typed every other time. A fallback moves the service away from the one
/// address the feature is about.
///
/// **And what held 8081 was this same application, three days older.** A
/// leftover `make serve DIST=dist-site` container from 2026-09-05 answered every
/// request at `:8081` while this server sat on an ephemeral port. The other
/// device got a *plausible* app — same UI, older code, predating both the
/// meet-message fix and the late-arm fix — so it did not look broken, it looked
/// like the product was broken. A wrong answer that renders correctly is worse
/// than a connection refused.
///
/// An explicit `port: 0` still means "any free port"; that is a caller asking,
/// not a fallback deciding. Callers that genuinely want to move should pass 0.
///
/// The error names the port, because *"could not start"* and *"could not start
/// because something else is on 8081"* send a person to different places.
pub fn start<R, F>(app: AppHandle<R>, port: u16, node_of: F) -> Result<AppServer, String>
where
    R: Runtime,
    F: Fn() -> Option<NodeHint> + Send + Sync + 'static,
{
    start_with_assets(Arc::new(EmbeddedAssets(app)), port, node_of)
}

/// [`start`] with the asset source injected — the seam the socket-level gate
/// enters through.
fn start_with_assets<F>(
    assets: Arc<dyn Assets>,
    port: u16,
    node_of: F,
) -> Result<AppServer, String>
where
    F: Fn() -> Option<NodeHint> + Send + Sync + 'static,
{
    let listener = std::net::TcpListener::bind(("0.0.0.0", port)).map_err(|e| {
        log::error!("app-server: cannot bind port {port} ({e}) — NOT serving");
        format!(
            "Port {port} is already in use, so the app is not being served. \
             Something else on this machine is answering there — often a leftover \
             `make serve`. Stop it and try again; anyone typing this machine's \
             address on port {port} is currently reaching that, not this app."
        )
    })?;
    let addr = listener.local_addr().map_err(|e| format!("app-server: local_addr: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let node_of = Arc::new(node_of);

    std::thread::Builder::new()
        .name("entity-app-server".into())
        .spawn(move || {
            for sock in listener.incoming() {
                if stop_thread.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(sock) = sock else { continue };
                let assets = assets.clone();
                let node_of = node_of.clone();
                // Thread per connection: a browser opens several in parallel for
                // the shell + wasm, and serving them serially would stall the
                // page on its own subresources.
                let _ = std::thread::Builder::new()
                    .name("entity-app-server-conn".into())
                    .spawn(move || serve_one(sock, assets.as_ref(), node_of().as_ref()));
            }
            log::info!("app-server: stopped");
        })
        .map_err(|e| format!("app-server: spawn failed: {e}"))?;

    log::info!("app-server: serving the SPA on {addr}");
    Ok(AppServer { addr, stop })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(peer: &str, addr: &str) -> NodeHint {
        NodeHint { peer_id: peer.into(), ws_addr: addr.into() }
    }

    /// The whole reason this server exists: a person types a bare host and the
    /// browser arrives already knowing the rendezvous.
    #[test]
    fn a_bare_url_carries_the_node_so_nobody_retypes_a_peer_id() {
        let t = redirect_target(Some(&hint("2KaPtrxxhEmyi5", "ws://192.168.68.55:4041")))
            .expect("a serving node yields a target");
        assert_eq!(
            t,
            "/?webrtc_node_peer=2KaPtrxxhEmyi5&webrtc_node=ws%3A%2F%2F192.168.68.55%3A4041",
            "both halves, and the address encoded so a parser cannot truncate it at the colon",
        );
    }

    /// `resolve_webrtc_provisioning` yields `None` for a half config, so half a
    /// hint installs no establisher while the URL claims one was provisioned.
    #[test]
    fn half_a_node_is_no_node() {
        assert!(redirect_target(None).is_none());
        assert!(redirect_target(Some(&hint("", "ws://x:4041"))).is_none());
        assert!(redirect_target(Some(&hint("2Ka", ""))).is_none());
        assert!(redirect_target(Some(&hint("  ", "  "))).is_none());
    }

    /// Opt in to immutable. The inverse default is what put a one-year cache on
    /// the file carrying the registry pin.
    ///
    /// **The filenames here are copied verbatim out of a real `dist/`, not
    /// invented.** The first version of this test made them up, missed
    /// wasm-bindgen's `_bg` suffix, and so passed while the actual 29 MB wasm
    /// — the largest thing this server serves — was classified `no-store`.
    const IMMUTABLE: &str = "public, max-age=31536000, immutable";

    #[test]
    fn only_a_content_hashed_bundle_is_cached_forever() {
        // Real names, `make wasm` output, 2026-08-21.
        assert_eq!(cache_control("/entity-browser-84dedeb6fa50b5bf_bg.wasm"), IMMUTABLE);
        assert_eq!(cache_control("/entity-browser-84dedeb6fa50b5bf.js"), IMMUTABLE);
        // Content is addressed by its hash, so the path is the version — and
        // the SHARD must be the hash's own first four characters, which is what
        // separates the blob store from an ingested site's `content/` directory
        // (C15; `/content/ab/cd/abcd1234` used to pass here and is now correctly
        // mutable, because eight characters is not a hash).
        assert_eq!(
            cache_control("/content/00/ca/00cae3408b6ed7ad12be0cde47e2957f768f252ac20e0afdf7b70dda5812b66ac0"),
            IMMUTABLE
        );
        assert_eq!(cache_control("/content/ab/cd/abcd1234"), "no-store");
        // A prefixed deployment keeps its immutable blobs — `starts_with` lost
        // these, which is the safe-but-costly direction this rule also fixes.
        assert_eq!(
            cache_control("/docs/content/00/ca/00cae3408b6ed7ad12be0cde47e2957f768f252ac20e0afdf7b70dda5812b66ac0"),
            IMMUTABLE
        );
        // An ingested Hugo/Zola site's own `content/` tree is mutable HTML at a
        // stable URL. This is the case that produces a deployment nobody can
        // correct for a year.
        assert_eq!(cache_control("/2K9hB/sites/blog/content/about.html"), "no-store");

        // The shell and anything carrying deployment posture must revalidate —
        // note both of these are real `dist/` entries too, and neither is hashed.
        assert_eq!(cache_control("/index.html"), "no-store");
        assert_eq!(cache_control("/entity-deployment.json"), "no-store");
        assert_eq!(cache_control("/sw.js"), "no-store");
        assert_eq!(cache_control("/entity-worker_bg.wasm"), "no-store", "unhashed: no hex run");
        assert_eq!(cache_control("/entity-worker-loader.js"), "no-store");

        // A hand-written name that merely contains a dash is not a hash.
        assert_eq!(cache_control("/my-worker.js"), "no-store");
        assert_eq!(cache_control("/a-1a2b3c.js"), "no-store", "6 hex is under the 8 floor");
        assert_eq!(cache_control("/a-zzzzzzzz.js"), "no-store", "not hex");
        assert_eq!(cache_control("/-1a2b3c4d.js"), "no-store", "no head: not an artifact name");
    }

    /// **C15's cross-tree pin.** The app crate asserts the same file; this side
    /// asserts it too, so the `#[path]` include cannot silently stop resolving
    /// to the rule everyone else is testing. If this red-lines and the app
    /// crate's twin does not, the two trees have diverged again — which is the
    /// whole failure this arrangement replaced.
    #[test]
    fn the_desktop_server_agrees_with_the_shared_cache_vectors() {
        const VECTORS: &str = include_str!("../../tools/cache-policy-vectors.txt");
        const IMMUTABLE_CC: &str = "public, max-age=31536000, immutable";
        let mut n = 0;
        let mut wrong = Vec::new();
        for line in VECTORS.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (expect, path) = line.split_once(char::is_whitespace).expect("malformed vector");
            let path = path.trim();
            let want = match expect {
                "immutable" => IMMUTABLE_CC,
                "mutable" => "no-store",
                other => panic!("bad expectation {other:?}"),
            };
            n += 1;
            if cache_control(path) != want {
                wrong.push(format!("{path:?}: wanted {want}, got {}", cache_control(path)));
            }
        }
        assert!(n >= 20, "only {n} vectors reached this side — the include path or the file moved");
        assert!(wrong.is_empty(), "desktop server disagrees with the shared vectors:\n{wrong:#?}");
    }

    #[test]
    fn only_get_and_head_are_answered() {
        let r = parse_request_line("GET / HTTP/1.1").unwrap();
        assert_eq!((r.method, r.path.as_str(), r.had_query), (Method::Get, "/", false));
        let r = parse_request_line("HEAD /x.js HTTP/1.1").unwrap();
        assert_eq!((r.method, r.path.as_str(), r.had_query), (Method::Head, "/x.js", false));
        assert!(parse_request_line("POST / HTTP/1.1").is_err());
        assert!(parse_request_line("DELETE /x HTTP/1.1").is_err());
        assert!(parse_request_line("").is_err());
        // Absolute-form and garbage targets are refused rather than normalized.
        assert!(parse_request_line("GET http://evil/ HTTP/1.1").is_err());
    }

    /// The asset map is keyed on paths; a query string is not part of the key.
    /// This is also what keeps the redirect from looping — the parameterized
    /// URL resolves to `/` and must then be served, not redirected again.
    /// The asset map is keyed on paths; a query string is not part of the key.
    /// But `had_query` must survive the stripping, because it is the only thing
    /// that distinguishes the URL we redirect *to* from the one we redirect
    /// *from* — they have the same path.
    #[test]
    fn a_query_string_is_not_part_of_the_asset_key_but_is_still_remembered() {
        let r = parse_request_line("GET /?webrtc_node_peer=abc&webrtc_node=ws HTTP/1.1").unwrap();
        assert_eq!(r.path, "/");
        assert!(r.had_query, "or the redirect loops forever");
        let r = parse_request_line("GET /index.html#frag HTTP/1.1").unwrap();
        assert_eq!(r.path, "/index.html");
        assert!(!r.had_query, "a fragment is not a query");
    }

    // ---------------------------------------------------------------------
    // Socket-level gates. Everything above is a pure function; none of it can
    // tell a server that answers from one that binds and says nothing.
    // ---------------------------------------------------------------------

    struct FakeAssets(Vec<(&'static str, &'static str, &'static str)>);

    impl Assets for FakeAssets {
        fn get(&self, path: &str) -> Option<(Vec<u8>, String)> {
            self.0
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, body, mime)| (body.as_bytes().to_vec(), (*mime).to_string()))
        }
    }

    fn fixture() -> Arc<dyn Assets> {
        Arc::new(FakeAssets(vec![
            ("/index.html", "<!doctype html><title>Tori</title>", "text/html"),
            ("/entity-browser-84dedeb6fa50b5bf_bg.wasm", "\0asm-ish", "application/wasm"),
        ]))
    }

    /// One request, one response, socket closed. Returns the raw bytes so the
    /// assertions can be about the wire and not about our own helpers.
    fn request(port: u16, line: &str) -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        write!(s, "{line}\r\nHost: localhost\r\n\r\n").expect("write");
        let mut out = Vec::new();
        s.read_to_end(&mut out).expect("read");
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The gate the pure tests cannot be: bind a real socket, speak real
    /// HTTP, and check what comes back.
    #[test]
    fn the_server_answers_a_real_request_over_a_real_socket() {
        let node = hint("2KaNODE", "ws://192.168.1.9:4041");
        let server = start_with_assets(fixture(), 0, move || Some(node.clone()))
            .expect("bind an ephemeral port");
        let port = server.port();

        // A bare `/` hands the visitor the rendezvous without them typing it.
        let r = request(port, "GET / HTTP/1.1");
        assert!(r.starts_with("HTTP/1.1 302 "), "bare / redirects, got: {}", &r[..r.len().min(60)]);
        assert!(r.contains("webrtc_node_peer=2KaNODE"), "the node rides the redirect: {r}");

        // …and the redirected-to URL must SERVE, not redirect again, or the
        // browser loops forever and the page never loads.
        let r = request(port, "GET /?webrtc_node_peer=2KaNODE&webrtc_node=ws HTTP/1.1");
        assert!(r.starts_with("HTTP/1.1 200 OK"), "the parameterized URL serves: {r}");
        assert!(r.contains("<title>Tori</title>"), "and serves index.html: {r}");
        assert!(r.contains("Cache-Control: no-store"), "the shell must revalidate: {r}");

        // The hashed bundle is the one thing pinned forever.
        let r = request(port, "GET /entity-browser-84dedeb6fa50b5bf_bg.wasm HTTP/1.1");
        assert!(r.contains("Content-Type: application/wasm"), "{r}");
        assert!(r.contains("max-age=31536000, immutable"), "{r}");

        // HEAD carries the headers and no body.
        let r = request(port, "HEAD /entity-browser-84dedeb6fa50b5bf_bg.wasm HTTP/1.1");
        assert!(r.contains("Content-Length: 0"), "{r}");

        assert!(request(port, "GET /nope HTTP/1.1").starts_with("HTTP/1.1 404 "));
        assert!(request(port, "POST / HTTP/1.1").starts_with("HTTP/1.1 405 "));
        assert!(request(port, "GARBAGE").starts_with("HTTP/1.1 400 "));
    }

    /// With no rendezvous the app must still be served. Provisioning is a
    /// bonus; a browser that cannot load the page at all is a regression.
    #[test]
    fn with_no_rendezvous_the_app_is_still_served() {
        let server = start_with_assets(fixture(), 0, || None).expect("bind");
        let r = request(server.port(), "GET / HTTP/1.1");
        assert!(r.starts_with("HTTP/1.1 200 OK"), "no node, no redirect, still the app: {r}");
        assert!(r.contains("<title>Tori</title>"));
    }

    /// The hint is consulted per request, not captured at bind: a person turns
    /// the rendezvous on *after* starting the server, and the next visitor must
    /// get it. Capturing would make this pass only if you toggled first.
    #[test]
    fn the_rendezvous_can_be_turned_on_after_the_server_is_already_up() {
        let on = Arc::new(AtomicBool::new(false));
        let flag = on.clone();
        let server = start_with_assets(fixture(), 0, move || {
            flag.load(Ordering::Relaxed).then(|| hint("2KaLATE", "ws://10.0.0.4:4041"))
        })
        .expect("bind");
        let port = server.port();

        assert!(request(port, "GET / HTTP/1.1").starts_with("HTTP/1.1 200 OK"));
        on.store(true, Ordering::Relaxed);
        let r = request(port, "GET / HTTP/1.1");
        assert!(r.starts_with("HTTP/1.1 302 "), "the later visitor gets the node: {r}");
        assert!(r.contains("webrtc_node_peer=2KaLATE"), "{r}");
    }

    /// **The inverse of the test this replaces**, which asserted the server
    /// moved to an ephemeral port and called that the better answer. It is not:
    /// this server exists so a person can type a URL on *another* device, and
    /// moving puts it somewhere they will not type. Measured 2026-09-07 — a
    /// leftover `make serve` held 8081 and served a three-day-old build to the
    /// other device for two sessions, looking like a product bug.
    ///
    /// The error must NAME the port, or "could not start" sends the reader
    /// looking in the wrong place.
    #[test]
    fn a_taken_port_fails_loudly_and_names_the_port() {
        let blocker = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let taken = blocker.local_addr().unwrap().port();
        // `expect_err` would need `AppServer: Debug`; matching keeps the handle
        // out of the failure path and drops any accidental server immediately.
        let err = match start_with_assets(fixture(), taken, || None) {
            Ok(s) => panic!(
                "a taken port must NOT be served around — it bound {} instead",
                s.port()
            ),
            Err(e) => e,
        };
        assert!(
            err.contains(&taken.to_string()),
            "the error must name the port that is taken, got: {err}",
        );
        drop(blocker);
    }

    /// An explicit `0` is a caller asking for any free port, which is a
    /// different thing from a fallback deciding to move. It must keep working —
    /// every other test in this module relies on it.
    #[test]
    fn port_zero_is_still_an_explicit_request_for_any_port() {
        let server = start_with_assets(fixture(), 0, || None).expect("port 0 binds");
        assert_ne!(server.port(), 0, "the OS assigned a real port");
        assert!(request(server.port(), "GET / HTTP/1.1").starts_with("HTTP/1.1 200 OK"));
    }

    #[test]
    fn a_redirect_is_never_cached() {
        let bytes = serialize(&Reply::Redirect("/?x=1".into()));
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("HTTP/1.1 302 Found\r\n"), "302, not 301 — the node can change");
        assert!(s.contains("Location: /?x=1\r\n"));
        assert!(s.contains("Cache-Control: no-store\r\n"));
    }
}

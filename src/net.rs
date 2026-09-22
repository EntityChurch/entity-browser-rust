//! Bounded network reads — the one place a fetch on the boot path may happen.
//!
//! # Why this module exists
//!
//! **D23: no unbounded network await on the boot path.** Between page load and
//! the frame loop running, every network operation is bounded by an explicit
//! deadline, and exceeding the deadline is a **state the boot proceeds from**,
//! never a stall.
//!
//! The distinction that makes this necessary is not "online vs offline". A
//! network that **rejects** — interface down, DNS failure, connection refused —
//! rejects the promise promptly, so every `.catch` and every `.ok()?` on the
//! path is reached and boot continues. That case has always worked, and it is
//! why the symptom was intermittent and easy to disbelieve.
//!
//! A network that **accepts and never answers** — a captive portal, a half-open
//! socket, a foreign LAN blackholing an address that used to work, an
//! overloaded CDN edge — never rejects anything. A bare `JsFuture::from(
//! window.fetch_with_str(url)).await` against it does not fail; it does not
//! return at all, for as long as the OS is willing to wait (75–130 s on Linux,
//! effectively unbounded behind a portal). If that await sits before the rAF
//! loop starts, the result is a permanently blank page **with no frozen-frame
//! watchdog**, because the watchdog installs after boot returns. No banner, no
//! message, no exit. That is brick-matrix cell #1 at E6, and it was live on the
//! build both production domains were serving.
//!
//! Three instances of the same shape had been found in three subsystems sharing
//! no code (`sw.js` `networkFirst`, this fetch, and the worker build-key skew),
//! which is what promoted the rule from an anti-pattern to a discipline.
//!
//! # The contract
//!
//! `fetch_text_bounded` bounds the **whole read**, headers *and* body, under one
//! deadline. Bounding only the header phase would leave the identical hazard one
//! step later: an origin may answer `200`, hand over headers, and then never
//! send a body. Returning a `Response` and letting the caller read it would put
//! the body read outside the deadline, so this module deliberately does not
//! expose one.
//!
//! A timeout is reported as `None` — the same value as "not served" — because
//! every caller on the boot path must already have a defined behaviour for
//! "there is no document here" (D16: best-effort, never blocks or fails boot),
//! and a timeout is that case. A caller that would do something *different* on a
//! timeout than on a 404 is a caller that wants to stall.
//!
//! # Enforcement
//!
//! `tools/net-lint.sh` (in `make lint`) is the grep gate: a raw
//! `fetch_with_str` / `fetch_with_request` outside this module and the
//! allowlisted non-boot call sites is a lint failure. The behavioural gate is
//! `boot_survives_a_blackholed_deployment_config` in `tests/e2e_worker.rs`
//! (G1), which serves from a socket that accepts and never responds. Neither
//! alone is sufficient: the lint cannot tell whether a deadline is honoured, and
//! the gate cannot tell whether a *new* fetch was added somewhere it does not
//! look.

/// Default deadline for a boot-path read, in milliseconds.
///
/// **3 s, and the number is borrowed rather than invented.** Workbox — the
/// reference implementation of network-first, whose `networkTimeoutSeconds`
/// option we hand-rolled without — ships `networkTimeoutSeconds: 3` for
/// navigations in its `pageCache()` recipe, on the documented rationale that
/// without it a network-first strategy "will wait indefinitely for a network
/// response even when the user is offline". Our boot fetch is a same-origin GET
/// of a ~400-byte document served `max-age=0, must-revalidate`, measured on all
/// four production apexes, so 3 s is roughly two orders of magnitude of headroom
/// over the healthy case on any network that is answering at all.
///
/// The cost of it being too short is a boot that uses build-time defaults for
/// one load and picks the config up on the next. The cost of it being too long
/// is the blank page. Those are not symmetric, and the constant is set knowing
/// which way to err.
#[cfg(target_arch = "wasm32")]
pub const BOOT_FETCH_DEADLINE_MS: i32 = 3_000;

/// The outcome of a bounded read. `text` is empty unless `ok`.
#[cfg(target_arch = "wasm32")]
pub struct BoundedResponse {
    pub status: u16,
    pub ok: bool,
    pub text: String,
}

/// Clears the deadline timer and drops its closure on **every** exit path —
/// success, HTTP error, parse failure, or early `?`.
///
/// Not a nicety. A leaked `setTimeout` would abort an `AbortController` that
/// nothing is waiting on (harmless), but the leaked `Closure` is permanent: this
/// repo's standing rule is that closures are owned and freed, never
/// `Closure::forget()`-ed, because a leak here is per-boot and unbounded.
#[cfg(target_arch = "wasm32")]
struct DeadlineGuard {
    window: web_sys::Window,
    timer: i32,
    _on_deadline: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        self.window.clear_timeout_with_handle(self.timer);
    }
}

/// `GET url`, read the body, and give up after `deadline_ms` — headers and body
/// under one deadline.
///
/// Returns `None` for every failure, including the deadline: unreachable,
/// aborted, not a `Response`, or a body that is not text. `Some` with
/// `ok == false` means the origin *answered* with a non-2xx, which is
/// information (the document is not served here) rather than a failure to reach
/// it, so it is reported rather than flattened.
#[cfg(target_arch = "wasm32")]
pub async fn fetch_text_bounded(url: &str, deadline_ms: i32) -> Option<BoundedResponse> {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window()?;
    let controller = web_sys::AbortController::new().ok()?;
    let signal = controller.signal();

    // Arm the deadline BEFORE the fetch is issued. `abort()` rejects the
    // in-flight promise — and, per spec, tears down the body stream too, which
    // is what makes one signal cover both phases.
    let on_deadline = Closure::wrap(Box::new(move || {
        controller.abort_with_reason(&JsValue::from_str("entity-browser: boot fetch deadline"));
    }) as Box<dyn FnMut()>);
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            on_deadline.as_ref().unchecked_ref(),
            deadline_ms,
        )
        .ok()?;
    let _guard = DeadlineGuard {
        window: window.clone(),
        timer,
        _on_deadline: on_deadline,
    };

    let opts = web_sys::RequestInit::new();
    opts.set_signal(Some(&signal));
    let resp_val = JsFuture::from(window.fetch_with_str_and_init(url, &opts))
        .await
        .ok()?;
    let resp: web_sys::Response = resp_val.dyn_into().ok()?;
    let status = resp.status();
    let ok = resp.ok();
    if !ok {
        return Some(BoundedResponse {
            status,
            ok,
            text: String::new(),
        });
    }
    // Still inside the deadline: an origin that sends headers and then stalls
    // the body is the same failure one step later.
    let text = JsFuture::from(resp.text().ok()?).await.ok()?.as_string()?;
    Some(BoundedResponse { status, ok, text })
}

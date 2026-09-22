//! DOM helper utilities — reduce web-sys boilerplate.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element};

pub use crate::window::ClosureVec;

/// Context for DOM event handlers in window views.
///
/// Bundles the action queue, repaint signal, closure storage, and window ID
/// so view code doesn't carry this boilerplate on every handler.
///
/// Common patterns are one-liners:
/// ```ignore
/// ctx.on_window_event(&btn, "click", "set_mode", "guided");
/// ctx.on_select_change(&select, "select_peer");
/// ctx.on_action(&btn, "click", Action::ClearEventLog);
/// ```
///
/// For complex handlers that need custom logic, use `ctx.listen()`.
/// Per-section in-flight text-input values, keyed by `field_id` and
/// owned by the renderer's `WindowSectionState`. The map's lifetime
/// is tied to the window section: closing the window drops it.
///
/// Used by [`tracked_input`] / [`tracked_textarea`] so user typing
/// survives section rebuilds (exec-completion, scrollback appends,
/// etc.) without each window having to re-implement the per-keystroke
/// model.draft pattern.
pub type DraftsMap = Rc<RefCell<std::collections::HashMap<String, String>>>;

#[derive(Clone)]
pub struct DomCtx {
    pub window_id: crate::window::WindowId,
    pub actions: Rc<RefCell<Vec<crate::action::Action>>>,
    pub repaint: crate::window::RepaintFn,
    pub closures: ClosureVec,
    /// In-flight input drafts for this window section. New
    /// `tracked_input`/`tracked_textarea` helpers read+write this so
    /// typing isn't clobbered when the section rebuilds (subscription
    /// fires, async result lands). Untouched by traditional manual
    /// inputs; opt-in via the helpers.
    pub drafts: DraftsMap,
    /// App-owned in-memory dial transients (a dial in flight / a dial that gave
    /// up before ever connecting) — the kernel liveness surface does not model
    /// these, so the display models read them from here at render time rather
    /// than from a tree store (`crate::dial_markers`). A cheap Arc-clone handle:
    /// one owner (the app), read here, never a per-window copy.
    pub dial_markers: crate::dial_markers::DialMarkers,
    /// Outcome of the last manual Connect press (`crate::connect_attempt`) —
    /// the surface that stops `Action::ConnectPeer` from completing silently.
    /// Same ownership shape as `dial_markers`: one app-owned handle, read at
    /// render. Distinct from it because a manual connect is keyed by the
    /// *address* typed, which has no peer id until the handshake lands.
    pub connect_attempt: crate::connect_attempt::ConnectAttempt,
    /// Outcome of the last file offered (`crate::offer_attempt`) — the surface
    /// that stops `Action::OfferFile` from completing silently. Same ownership
    /// shape as the two above: one app-owned handle, read at render.
    ///
    /// Unlike them it is also **written from the DOM**, and deliberately: the
    /// file picker and the `array_buffer()` read both sit between the tap and
    /// the action, so a refusal on size or a failed read has no action to ride
    /// and would otherwise be invisible to the app entirely.
    pub offer_attempt: crate::offer_attempt::OfferAttempt,
    /// Outcome of the last Pull (`crate::pull_attempt`) — same ownership shape
    /// as `offer_attempt`, written only from the app side.
    pub pull_attempt: crate::pull_attempt::PullAttempt,
    /// Would a reload change the §6.5 signaling node this session rendezvous
    /// through? A plain fact rather than a handle, because unlike the two above
    /// it is *derived* per frame (boot-time provisioning vs what a fresh resolve
    /// would return) and there is no state for a view to poke.
    ///
    /// It exists because `InitParams.webrtc` is Init-only upstream: choosing a
    /// connector mid-session cannot reach the running establisher, so the choice
    /// is pending until reload and the UI has to say so.
    pub provisioning_drifted: bool,
}

impl DomCtx {
    /// Attach a handler that pushes a WindowEvent with static event name and value.
    /// Covers the most common case — a button/radio/checkbox that sets a field.
    pub fn on_window_event(&self, el: &Element, dom_event: &str, event: &str, value: &str) {
        let actions = self.actions.clone();
        let rp = self.repaint.clone();
        let wid = self.window_id;
        let ev: String = event.into();
        let val: String = value.into();
        listen(el, dom_event, move |_| {
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id: wid,
                event: ev.clone(),
                value: val.clone(),
            });
            rp();
        }, &self.closures);
    }

    /// Attach a change handler on a `<select>` that pushes a WindowEvent
    /// with the selected value.
    pub fn on_select_change(&self, el: &Element, event: &str) {
        let actions = self.actions.clone();
        let rp = self.repaint.clone();
        let wid = self.window_id;
        let ev: String = event.into();
        let sel_ref = el.clone();
        listen(el, "change", move |_| {
            let val = sel_ref
                .dyn_ref::<web_sys::HtmlSelectElement>()
                .map(|s| s.value())
                .unwrap_or_default();
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id: wid,
                event: ev.clone(),
                value: val,
            });
            rp();
        }, &self.closures);
    }

    /// Attach a handler that pushes a specific action.
    pub fn on_action(&self, el: &Element, dom_event: &str, action: crate::action::Action) {
        let actions = self.actions.clone();
        let rp = self.repaint.clone();
        listen(el, dom_event, move |_| {
            actions.borrow_mut().push(action.clone());
            rp();
        }, &self.closures);
    }

    /// Attach a custom event handler. Use for complex handlers that need
    /// to read DOM state, push multiple actions, or have conditional logic.
    pub fn listen(&self, el: &Element, dom_event: &str, handler: impl Fn(web_sys::Event) + 'static) {
        listen(el, dom_event, handler, &self.closures);
    }
}

/// Register an event listener and keep the closure alive in the given storage.
/// Prefer `DomCtx` methods over calling this directly in view code.
pub fn listen(
    el: &Element,
    event: &str,
    handler: impl Fn(web_sys::Event) + 'static,
    closures: &ClosureVec,
) {
    let closure = Closure::wrap(Box::new(handler) as Box<dyn Fn(web_sys::Event)>);
    el.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())
        .ok();
    closures.borrow_mut().push(closure.into_js_value());
}

pub fn document() -> Document {
    web_sys::window().unwrap().document().unwrap()
}

// --- Fullscreen API ---------------------------------------------------------
//
// Reached through `Reflect` rather than `web_sys`, and the reason is not style:
// `Element::request_fullscreen` is typed `-> Result<(), JsValue>` in web-sys
// 0.3, so it **calls the method and drops the Promise the method returns**. That
// Promise REJECTS whenever the request is refused — no transient activation, an
// element the engine will not promote, a WebView built with fullscreen off — and
// a dropped rejecting Promise is the standing footgun in this repo (`index.html`
// warns on a post-start `unhandledrejection`, and reloads the whole app on a
// pre-start one). Calling through `Reflect` hands us the return value so we can
// consume it. The same applies to `Document::exit_fullscreen`, typed `-> ()`.
//
// The prefixed names are asked for second because WebKitGTK — the Tauri Linux
// WebView — shipped the unprefixed API only recently, and the cost of a second
// `Reflect::get` on a path that runs once per button press is nothing.

/// Invoke the first of `names` that exists on `target`, consuming a returned
/// Promise so a refusal never surfaces as an unhandled rejection. Returns
/// whether a method was found and called (NOT whether the transition happened —
/// that is asynchronous, and the only honest observer of it is
/// `fullscreenchange`).
fn call_consuming_promise(target: &wasm_bindgen::JsValue, names: &[&str]) -> bool {
    for name in names {
        let Ok(prop) = js_sys::Reflect::get(target, &wasm_bindgen::JsValue::from_str(name)) else {
            continue;
        };
        let Ok(f) = prop.dyn_into::<js_sys::Function>() else {
            continue;
        };
        let Ok(ret) = f.call0(target) else {
            return false;
        };
        if let Ok(promise) = ret.dyn_into::<js_sys::Promise>() {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            });
        }
        return true;
    }
    false
}

/// Open a file input's chooser, preferring `showPicker()` **because it
/// reports**. `Ok(())` means the engine accepted the request; `Err(reason)` is
/// a refusal with the engine's own words.
///
/// **The whole point is the return type.** `HTMLElement::click()` on a hidden
/// file input is fire-and-forget: if the engine declines to open a chooser
/// there is no dialog, no `change` event, no exception and no console entry —
/// which is exactly how this was reported from Android/Firefox, *"a broken ass
/// button"* that works on desktop. `showPicker()` throws instead
/// (`NotAllowedError` without transient activation, `InvalidStateError`,
/// `SecurityError`), so the same failure arrives as a sentence we can put on
/// screen. It is also the API specified for programmatic opening, where
/// `.click()` is a workaround that predates it.
///
/// `.click()` remains the fallback for engines older than `showPicker`
/// (Firefox 101+, Chrome 99+, Safari 16+), so nothing that works today stops
/// working. **A caller must surface the `Err`** — swallowing it rebuilds the
/// exact silence this exists to remove.
pub fn show_file_picker(el: &web_sys::HtmlElement) -> Result<(), String> {
    let target: wasm_bindgen::JsValue = el.clone().into();
    if let Ok(prop) = js_sys::Reflect::get(&target, &wasm_bindgen::JsValue::from_str("showPicker"))
    {
        if let Ok(f) = prop.dyn_into::<js_sys::Function>() {
            return match f.call0(&target) {
                Ok(_) => Ok(()),
                // `showPicker` throws synchronously; there is no promise to
                // consume, so this cannot become an unhandled rejection.
                Err(e) => Err(describe_js_error(&e)),
            };
        }
    }
    el.click();
    Ok(())
}

/// A thrown JS value as something a person can read. A `DOMException` carries a
/// `name` and a `message`; anything else falls back to its debug form, which is
/// still better than nothing on screen.
pub fn describe_js_error(e: &wasm_bindgen::JsValue) -> String {
    let get = |k: &str| {
        js_sys::Reflect::get(e, &wasm_bindgen::JsValue::from_str(k))
            .ok()
            .and_then(|v| v.as_string())
            .filter(|s| !s.is_empty())
    };
    match (get("name"), get("message")) {
        (Some(n), Some(m)) => format!("{n}: {m}"),
        (Some(n), None) => n,
        (None, Some(m)) => m,
        _ => format!("{e:?}"),
    }
}

/// Can this document take an element fullscreen at all? A WebView embedder may
/// compile the API out or switch it off; a button that cannot work must not be
/// rendered rather than press silently.
pub fn fullscreen_supported() -> bool {
    let doc: wasm_bindgen::JsValue = document().into();
    for name in ["fullscreenEnabled", "webkitFullscreenEnabled"] {
        if let Ok(v) = js_sys::Reflect::get(&doc, &wasm_bindgen::JsValue::from_str(name)) {
            if v.as_bool() == Some(true) {
                return true;
            }
        }
    }
    false
}

/// Ask for `el` to fill the screen. Must be called from a user gesture.
pub fn request_fullscreen(el: &Element) {
    let target: wasm_bindgen::JsValue = el.clone().into();
    call_consuming_promise(&target, &["requestFullscreen", "webkitRequestFullscreen"]);
}

/// Leave fullscreen, whichever element is in it.
pub fn exit_fullscreen() {
    let target: wasm_bindgen::JsValue = document().into();
    call_consuming_promise(&target, &["exitFullscreen", "webkitExitFullscreen"]);
}

/// Is `el` the element currently filling the screen?
///
/// Asked as `el.matches(":fullscreen")`, **not** by comparing against
/// `document.fullscreenElement`: our windows render inside a shadow root, and
/// that property retargets to the shadow *host*, so the comparison is false for
/// every element we would ever ask about.
pub fn is_fullscreen(el: &Element) -> bool {
    for sel in [":fullscreen", ":-webkit-full-screen"] {
        // `matches` throws on a selector the engine does not know, which is the
        // signal to try the other spelling rather than an error.
        if let Ok(true) = el.matches(sel) {
            return true;
        }
    }
    false
}

pub fn create_element(tag: &str) -> Element {
    document().create_element(tag).unwrap()
}

pub fn create_element_with_class(tag: &str, class: &str) -> Element {
    let el = create_element(tag);
    el.set_class_name(class);
    el
}

pub fn set_text(el: &Element, text: &str) {
    el.set_text_content(Some(text));
}

/// Escape a string for safe interpolation into an HTML string passed
/// to `set_inner_html`. Use this for **every** data value spliced into
/// an innerHTML template — peer ids, labels, addresses, payloads, log
/// messages — even when the value looks hash-like today (the moment a
/// field carries user/remote text it becomes an injection vector).
/// Do NOT use it on intentional markup (e.g. generated SVG).
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn set_attr(el: &Element, name: &str, value: &str) {
    el.set_attribute(name, value).unwrap();
}

pub fn append(parent: &Element, child: &Element) {
    parent.append_child(child).unwrap();
}

/// Insert `child` as the FIRST child of `parent` (so it renders at the top of
/// the stack). Used for new window sections so the newest window shows above
/// the rest instead of pushed to the bottom of a scrolled container.
pub fn prepend(parent: &Element, child: &Element) {
    let _ = parent.insert_before(child, parent.first_child().as_ref());
}

pub fn clear_children(el: &Element) {
    el.set_inner_html("");
}

/// Create an `<input type="text">` whose value survives section
/// rebuilds. The element's value is sourced from the section's
/// drafts map (`ctx.drafts`) when `field_id` is present there;
/// otherwise it falls back to `initial`. Each keystroke fires an
/// `input` event that writes the live value into the drafts map —
/// without going through the action queue, so no rebuild is
/// triggered and other windows aren't perturbed.
///
/// Use this in place of manually-constructed `<input>` elements
/// anywhere the user types more than a one-shot value (URI fields,
/// query filters, path inputs). The shell's prompt input uses the
/// same pattern but with its own model.draft because it also
/// participates in arrow-key history walking — that loop wants the
/// value reflected in model state, not just the renderer.
///
/// Reading the latest value at submit time: read from
/// `ctx.drafts.borrow().get(field_id)` (falling back to the form's
/// initial when absent).
pub fn tracked_input(
    parent: &Element,
    ctx: &DomCtx,
    field_id: &str,
    initial: &str,
    style: &str,
) -> Element {
    let input = tracked_input_el(ctx, field_id, initial, style);
    append(parent, &input);
    input
}

/// Detached [`tracked_input`]: builds the draft-tracked `<input>` without
/// appending it, so composers (`components::text_input`, `components::field`)
/// decide where it lands. Same mechanism, same drafts contract.
pub fn tracked_input_el(
    ctx: &DomCtx,
    field_id: &str,
    initial: &str,
    style: &str,
) -> Element {
    let value = ctx
        .drafts
        .borrow()
        .get(field_id)
        .cloned()
        .unwrap_or_else(|| initial.to_string());
    let input = create_element("input");
    input.set_attribute("type", "text").ok();
    input.set_attribute("value", &value).ok();
    input.set_attribute("data-field", field_id).ok();
    input.set_attribute("style", style).ok();

    // Track every keystroke directly into the drafts map. No action
    // dispatch, no save_state — just an in-memory write that
    // subsequent rebuilds read back. Without this, rebuilds clobber
    // the user's typing.
    let drafts = ctx.drafts.clone();
    let id = field_id.to_string();
    listen(
        &input,
        "input",
        move |evt: web_sys::Event| {
            let Some(target) =
                evt.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            drafts.borrow_mut().insert(id.clone(), target.value());
        },
        &ctx.closures,
    );

    input
}

/// Same as [`tracked_input`] but for multi-line text. The `<textarea>`'s
/// `value` is set via `set_text_content` (textareas hold content in
/// text rather than the `value` attribute).
pub fn tracked_textarea(
    parent: &Element,
    ctx: &DomCtx,
    field_id: &str,
    initial: &str,
    style: &str,
) -> Element {
    let value = ctx
        .drafts
        .borrow()
        .get(field_id)
        .cloned()
        .unwrap_or_else(|| initial.to_string());
    let area = create_element("textarea");
    area.set_attribute("data-field", field_id).ok();
    area.set_attribute("style", style).ok();
    area.set_text_content(Some(&value));

    let drafts = ctx.drafts.clone();
    let id = field_id.to_string();
    listen(
        &area,
        "input",
        move |evt: web_sys::Event| {
            let Some(target) = evt
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
            else {
                return;
            };
            drafts.borrow_mut().insert(id.clone(), target.value());
        },
        &ctx.closures,
    );

    append(parent, &area);
    area
}

/// Schedule an autoscroll-to-bottom of `el` after the current render
/// task finishes. The `setTimeout(0)` runs once the section has been
/// attached and the browser has computed dimensions, so `scrollTop =
/// scrollHeight` actually pins to the latest content. Use this on any
/// `<pre>`-style scrolling region that gets fresh content appended
/// (event log, query results, exec results, shell scrollback).
///
/// One-shot via `Closure::once_into_js`; no manual lifetime
/// management needed.
pub fn schedule_scroll_to_bottom(el: &Element) {
    use wasm_bindgen::closure::Closure;
    let el_for_scroll = el.clone();
    let cb = Closure::once_into_js(move || {
        if let Ok(p) = el_for_scroll.dyn_into::<web_sys::HtmlElement>() {
            p.set_scroll_top(p.scroll_height());
        }
    });
    if let Some(win) = web_sys::window() {
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            0,
        );
    }
}

pub fn get_element_by_id(id: &str) -> Option<Element> {
    document().get_element_by_id(id)
}

/// Capture the `scrollTop` of every `[data-scroll-key]` scroll container under
/// `root`, keyed by that stable attribute. A window rebuild tears down and
/// recreates the whole section (`clear_children` + re-render), so a scroller's
/// position is otherwise lost — jarringly snapping the user back to the top on
/// any tree expand / state change. Pair with [`restore_scroll_positions`]
/// around the rebuild; a view opts in by setting `data-scroll-key` on its
/// scrollable element.
pub fn capture_scroll_positions(root: &Element) -> Vec<(String, i32)> {
    let mut out = Vec::new();
    if let Ok(list) = root.query_selector_all("[data-scroll-key]") {
        for i in 0..list.length() {
            if let Some(el) = list.item(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                if let Some(key) = el.get_attribute("data-scroll-key") {
                    out.push((key, el.scroll_top()));
                }
            }
        }
    }
    out
}

/// Restore the scroll positions captured by [`capture_scroll_positions`], after
/// the section has been rebuilt and re-appended. Matches the freshly-created
/// scrollers by their `data-scroll-key`. Setting `scrollTop` past the (possibly
/// shorter, after a collapse) content height is harmless — the browser clamps.
pub fn restore_scroll_positions(root: &Element, saved: &[(String, i32)]) {
    for (key, top) in saved {
        if *top <= 0 {
            continue;
        }
        if let Ok(Some(el)) = root.query_selector(&format!("[data-scroll-key=\"{key}\"]")) {
            el.set_scroll_top(*top);
        }
    }
}

pub fn set_mode_class(mode_class: &str, mode_label: &str) {
    if let Some(container) = get_element_by_id("app-container") {
        container.set_class_name(mode_class);
    }
    if let Some(display) = get_element_by_id("mode-display") {
        set_text(&display, mode_label);
    }
}

/// Update the status-bar live status text (`#mode-display`) — the
/// at-a-glance summary (open windows · peers · durability). The caller
/// guards on a change first, so this only writes to the DOM on a real
/// delta rather than every frame.
pub fn set_status_text(text: &str) {
    if let Some(display) = get_element_by_id("mode-display") {
        set_text(&display, text);
    }
}

/// Set only the `#app-container` mode class (`mode-dom` chrome /
/// `mode-site` overlay) without touching the mode-display label. The
/// Site Mode toggle calls this when the active surface changes.
pub fn set_container_mode(mode_class: &str) {
    if let Some(container) = get_element_by_id("app-container") {
        container.set_class_name(mode_class);
    }
}

/// Show/hide the always-on status bar. Hidden in Site Mode so the
/// overlay fills the whole page (a real full-screen site) — the "back to
/// chrome" control then lives in the site's own nav bar, not here.
pub fn set_status_bar_visible(visible: bool) {
    if let Some(bar) = get_element_by_id("status-bar") {
        if let Some(html) = bar.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html
                .style()
                .set_property("display", if visible { "flex" } else { "none" });
        }
    }
}

/// Update the status-bar site toggle: visibility (`show` = the deployment
/// exposes the toggle *and* a site is available — no site ⇒ inert, the button
/// vanishes) + a **symbol** label (no words, so wording never drifts). A
/// strict content-site deployment / a no-site deployment passes `show = false`
/// and the button is gone. `active` only affects the title/glyph; in practice
/// the status bar is hidden while the overlay is active, so the visible state
/// is the "enter" one.
pub fn set_site_toggle(show: bool, active: bool) {
    if let Some(btn) = get_element_by_id("site-toggle") {
        if let Some(html) = btn.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html
                .style()
                .set_property("display", if show { "inline-block" } else { "none" });
        }
        // Symbol-only: ⛶ = enter the full-screen site surface; the way back
        // lives in the site's own nav bar ("Exit Site") while active.
        set_text(&btn, "\u{26f6}"); // ⛶ SQUARE FOUR CORNERS (full-screen)
        set_attr(&btn, "title", if active { "Exit site" } else { "View site" });
    }
}

/// Wire the always-on status-bar toggle (light DOM, outside the shadow
/// root) to push [`Action::ToggleSiteMode`](crate::action::Action) into
/// the shared action sink (drained each frame). The closure is kept
/// alive in `closures` (owned by the app for the page lifetime) per the
/// two-heap discipline (D12) — never `forget()`.
pub fn wire_site_toggle(
    sink: Rc<RefCell<Vec<crate::action::Action>>>,
    repaint: crate::window::RepaintFn,
    closures: &ClosureVec,
) {
    let Some(btn) = get_element_by_id("site-toggle") else {
        return;
    };
    listen(
        &btn,
        "click",
        move |_| {
            sink.borrow_mut().push(crate::action::Action::ToggleSiteMode);
            repaint();
        },
        closures,
    );
}


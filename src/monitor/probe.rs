//! What the browser will tell this page — read through `Reflect`, because most
//! of these APIs exist in one engine and not another, and an absent API must
//! read as *not available* rather than as zero (AP40).
//!
//! Measured support (`tools/monitor-probe`, Firefox 149 / Chrome 151):
//! `performance.memory`, `deviceMemory`, Long Animation Frames and
//! `PressureObserver` are Chromium-only; `hardwareConcurrency`,
//! `storage.estimate()` and WebAssembly memory are everywhere.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::views::system_monitor::output::{Engine, EngineFacts, JsHeap};

fn get(target: &JsValue, key: &str) -> Option<JsValue> {
    js_sys::Reflect::get(target, &JsValue::from_str(key))
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
}

fn global(key: &str) -> Option<JsValue> {
    get(&js_sys::global(), key)
}

/// Read once when a monitor opens.
pub fn engine_facts() -> EngineFacts {
    let navigator = global("navigator");
    let ua = navigator
        .as_ref()
        .and_then(|n| get(n, "userAgent"))
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    // Order matters: Chromium's UA also says "AppleWebKit" and "Safari".
    let engine = if ua.contains("Firefox/") {
        Engine::Firefox
    } else if ua.contains("Chrome/") || ua.contains("Chromium/") {
        Engine::Chromium
    } else if ua.contains("AppleWebKit") {
        Engine::WebKit
    } else {
        Engine::Unknown
    };
    let long_frames = global("PerformanceObserver")
        .and_then(|po| get(&po, "supportedEntryTypes"))
        .map(|types| js_sys::Array::from(&types).includes(&JsValue::from_str("long-animation-frame"), 0))
        .unwrap_or(false);
    EngineFacts {
        engine,
        cores: navigator.as_ref().and_then(|n| get(n, "hardwareConcurrency")).and_then(|v| v.as_f64()),
        device_memory_gb: navigator.as_ref().and_then(|n| get(n, "deviceMemory")).and_then(|v| v.as_f64()),
        cross_origin_isolated: global("crossOriginIsolated").and_then(|v| v.as_bool()).unwrap_or(false),
        timer_resolution_ms: timer_resolution(),
        long_frames,
        pressure_api: global("PressureObserver").is_some(),
    }
}

/// The smallest non-zero step `performance.now()` takes here. Browsers coarsen
/// it on purpose (a Spectre defence): measured 1 ms in Firefox and 0.1 ms in
/// Chrome without cross-origin isolation. 20 000 reads, once — well under a
/// millisecond of work.
fn timer_resolution() -> Option<f64> {
    let perf = web_sys::window()?.performance()?;
    let mut prev = perf.now();
    let mut smallest = f64::INFINITY;
    for _ in 0..20_000 {
        let now = perf.now();
        let step = now - prev;
        if step > 0.0 && step < smallest {
            smallest = step;
        }
        prev = now;
    }
    smallest.is_finite().then_some(smallest)
}

/// This app's WebAssembly linear memory, in bytes. Exact: we hold the object.
pub fn wasm_memory_bytes() -> Option<f64> {
    let memory = wasm_bindgen::memory();
    get(&memory, "buffer").and_then(|b| get(&b, "byteLength")).and_then(|v| v.as_f64())
}

/// `performance.memory` — Chromium only, rounded, and per renderer process
/// rather than per page.
pub fn js_heap() -> Option<JsHeap> {
    let memory = global("performance").and_then(|p| get(&p, "memory"))?;
    Some(JsHeap {
        used: get(&memory, "usedJSHeapSize")?.as_f64()?,
        limit: get(&memory, "jsHeapSizeLimit")?.as_f64()?,
    })
}

/// A live `PressureObserver` on "cpu", disconnected when dropped. Chromium only,
/// and it reports only while the page has focus.
pub struct PressureWatch {
    observer: JsValue,
    _callback: Closure<dyn FnMut(JsValue)>,
}

impl Drop for PressureWatch {
    fn drop(&mut self) {
        if let Some(disconnect) = get(&self.observer, "disconnect").and_then(|f| f.dyn_into::<js_sys::Function>().ok()) {
            let _ = disconnect.call0(&self.observer);
        }
    }
}

/// Start observing CPU pressure into `slot`, waking `dirty` on each change.
/// `None` where the API does not exist or refuses.
pub fn watch_pressure(slot: Rc<RefCell<Option<String>>>, dirty: crate::window_watch::DirtyFlag) -> Option<PressureWatch> {
    let ctor = global("PressureObserver")?.dyn_into::<js_sys::Function>().ok()?;
    let callback = Closure::wrap(Box::new(move |records: JsValue| {
        let records = js_sys::Array::from(&records);
        let last = records.get(records.length().saturating_sub(1));
        if let Some(state) = get(&last, "state").and_then(|s| s.as_string()) {
            let changed = slot.borrow().as_deref() != Some(state.as_str());
            *slot.borrow_mut() = Some(state);
            if changed {
                dirty.mark();
            }
        }
    }) as Box<dyn FnMut(JsValue)>);
    let args = js_sys::Array::of1(callback.as_ref().unchecked_ref());
    let observer = js_sys::Reflect::construct(&ctor, &args).ok()?;
    let observe = get(&observer, "observe")?.dyn_into::<js_sys::Function>().ok()?;
    let promise = observe.call1(&observer, &JsValue::from_str("cpu")).ok()?;
    // Consume the promise: a refusal (permissions policy, an unsupported
    // source) must not surface as an unhandled rejection.
    if let Ok(promise) = promise.dyn_into::<js_sys::Promise>() {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        });
    }
    Some(PressureWatch { observer, _callback: callback })
}

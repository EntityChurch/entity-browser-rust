//! `ops::gzip` — compress bytes with the browser's own `CompressionStream`:
//! `gzip` for a `.tar.gz`, `deflate-raw` for the members of a `.zip`.
//!
//! No crate: every engine this app runs on ships gzip (Firefox 113, Chrome 80,
//! Safari 16.4, WebKitGTK 2.42), and a compressor in the bundle would be bytes
//! every visitor downloads for a button few press. Reached through `Reflect`
//! rather than `web_sys` bindings, because `CompressionStream` sits behind
//! web-sys's unstable-API flag, and a flag that changes what compiles is a build
//! knob nobody remembers.
//!
//! An engine without it gets [`GzipError::Unsupported`], and the caller hands
//! over the uncompressed archive instead — a `.tar` still opens everywhere.

#![cfg(target_arch = "wasm32")]

use js_sys::{Array, Function, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GzipError {
    /// This engine has no `CompressionStream`.
    Unsupported,
    /// It has one and it failed.
    Failed(String),
}

fn get(target: &JsValue, key: &str) -> Result<JsValue, GzipError> {
    Reflect::get(target, &JsValue::from_str(key)).map_err(|e| GzipError::Failed(format!("{key}: {e:?}")))
}

fn call0(target: &JsValue, method: &str) -> Result<JsValue, GzipError> {
    let f: Function = get(target, method)?.dyn_into().map_err(|_| GzipError::Failed(format!("{method} is not a function")))?;
    f.call0(target).map_err(|e| GzipError::Failed(format!("{method}: {e:?}")))
}

pub async fn gzip(bytes: &[u8]) -> Result<Vec<u8>, GzipError> {
    compress("gzip", bytes).await // i18n-ignore — a format name
}

/// Raw DEFLATE (RFC 1951, no header) — what a zip member's method 8 holds.
/// Newer than `gzip` in some engines (Firefox 113, Chrome 103, Safari 16.4);
/// an engine without it gets [`GzipError::Unsupported`] and the zip stores.
pub async fn deflate_raw(bytes: &[u8]) -> Result<Vec<u8>, GzipError> {
    compress("deflate-raw", bytes).await // i18n-ignore — a format name
}

async fn compress(format: &str, bytes: &[u8]) -> Result<Vec<u8>, GzipError> {
    let global = js_sys::global();
    let ctor = get(&global, "CompressionStream")?;
    let Some(ctor) = ctor.dyn_ref::<Function>() else {
        return Err(GzipError::Unsupported);
    };
    let fail = |e: JsValue| GzipError::Failed(format!("{e:?}"));
    // An engine that has the constructor but not this format throws here.
    let cs = Reflect::construct(ctor, &Array::of1(&JsValue::from_str(format))).map_err(|_| GzipError::Unsupported)?;
    let blob = web_sys::Blob::new_with_u8_array_sequence(&Array::of1(&Uint8Array::from(bytes))).map_err(fail)?;
    let stream = call0(blob.as_ref(), "stream")?;
    let pipe: Function = get(&stream, "pipeThrough")?.dyn_into().map_err(|_| GzipError::Failed("pipeThrough".into()))?;
    let compressed = pipe.call1(&stream, &cs).map_err(fail)?;
    let response_ctor: Function = get(&global, "Response")?.dyn_into().map_err(|_| GzipError::Failed("Response".into()))?;
    let response = Reflect::construct(&response_ctor, &Array::of1(&compressed)).map_err(fail)?;
    let promise: js_sys::Promise = call0(&response, "arrayBuffer")?.dyn_into().map_err(|_| GzipError::Failed("arrayBuffer".into()))?;
    let buf = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(fail)?;
    Ok(Uint8Array::new(&buf).to_vec())
}

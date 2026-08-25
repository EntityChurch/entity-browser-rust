//! `ops::download` — materialize a pulled file onto the local device.
//!
//! A `local/files:read` returns the file entity plus an `included` map
//! carrying the content blob and (for small files) its chunks
//! (DOMAIN-LOCAL-FILES §4.1 / build_included). This module reassembles the
//! bytes from that map and hands them to the browser as a download — the
//! "get it onto the phone" step. WASM-only: the browser download API and
//! `EntityApp` are both wasm.
//!
//! Slice-0 scope: works when the response is self-contained (blob + all
//! chunks inline, i.e. small files). Larger files whose chunks are NOT
//! inlined surface a loud "chunk missing" error rather than a silent
//! partial write — the follow-up is to fetch the missing chunks via a
//! `system/content:get` round-trip against the same peer.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use entity_content::{blob_chunk_hashes, reassemble};
use entity_handler::HandlerResult;
use entity_store::{ContentStore, MemoryContentStore};

/// Reassemble the file bytes from a `local/files:read` result and trigger a
/// browser download. Returns the byte count on success.
pub fn materialize_and_download(result: &HandlerResult, filename: &str) -> Result<usize, String> {
    // Rebuild a content store from the response's included entities so the
    // shared reassembler can resolve the blob → chunks. Keyed by content
    // hash, matching the `included` map keys.
    let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    for entity in result.included.values() {
        // Best-effort: a put failure just means that hash won't resolve,
        // which reassemble() reports loudly below.
        let _ = store.put(entity.clone());
    }

    // Identify the blob: it's the one included entity that decodes as a
    // blob manifest. Chunks are raw payloads and fail this decode, so this
    // is robust without decoding the file entity's wrapped `content` field.
    let blob_hash = result
        .included
        .keys()
        .find(|h| blob_chunk_hashes(&store, h).is_ok())
        .copied()
        .ok_or_else(|| "no content blob in read response".to_string())?;

    let bytes = reassemble(&store, &blob_hash).map_err(|e| {
        format!(
            "reassemble failed ({e}) — the file's chunks may not be inlined \
             (large-file transfer is a follow-up)"
        )
    })?;

    let len = bytes.len();
    trigger_browser_download(filename, &bytes)?;
    Ok(len)
}

/// Hand `bytes` to the browser as a download named `filename`, using an
/// object-URL + a synthetic anchor click. Revokes the URL after.
fn trigger_browser_download(filename: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;

    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    // Blob from a [Uint8Array] sequence (no BlobPropertyBag needed — the
    // download attribute carries the filename/extension).
    let array = js_sys::Array::new();
    array.push(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&array)
        .map_err(|e| format!("blob create failed: {e:?}"))?;

    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| format!("object url failed: {e:?}"))?;

    let anchor = document
        .create_element("a")
        .map_err(|e| format!("create anchor failed: {e:?}"))?;
    anchor.set_attribute("href", &url).ok();
    anchor.set_attribute("download", filename).ok();
    // Click via HtmlElement — starts the download without appending to DOM.
    let el: web_sys::HtmlElement = anchor
        .dyn_into()
        .map_err(|_| "anchor is not an HtmlElement".to_string())?;
    el.click();

    // The download has been kicked off synchronously; free the URL.
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(())
}

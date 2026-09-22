//! The player's half of [`crate::apps::workspace`]: answer an app's
//! `x-work-list`, `x-work-get` and `x-work-save`.
//!
//! i18n-ignore-file — every string here is a refusal DETAIL that travels to the
//! app (which branches on the stable `reason` code) and to the log. Nothing in
//! this file renders text a person reads.
//!
//! Every rule is in `apps::workspace` and gated natively. This file only turns
//! messages into calls against the profile's [`WriterHandle`] and answers.
//! **Every declared request gets an answer**, including a refusal, and the frame
//! is stamped so a gate can count outcomes without reading into an opaque-origin
//! document.

use std::rc::Rc;

use wasm_bindgen::JsValue;

use crate::apps::workspace::{self, PutFile, Refusal, WorkStore, WorkspaceSource};
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

pub struct WorkspaceHost {
    source: WorkspaceSource,
    writer: WriterHandle,
    frame: web_sys::HtmlIFrameElement,
}

impl WorkspaceHost {
    /// `None` when there is no writer for this profile at all — the app is then
    /// told there is no workspace (`init` carries `false`), rather than handed
    /// one whose every save fails.
    pub fn mount(
        peers: &Peers,
        me: &str,
        source: WorkspaceSource,
        frame: &web_sys::HtmlIFrameElement,
    ) -> Option<Rc<Self>> {
        let Some(writer) = peers.writer_handle_for(me) else {
            tracing::warn!(peer = %me, app = %source.app_id, "app workspace: no writer handle — not offered");
            return None;
        };
        tracing::info!(app = %source.app_id, prefix = %source.prefix, "app workspace: mounted");
        Some(Rc::new(Self { source, writer, frame: frame.clone() }))
    }

    /// What `init` should say: `true` only when a save could actually land.
    pub fn usable(&self) -> bool {
        match self.writer.usable() {
            Ok(()) => true,
            Err(why) => {
                tracing::warn!(app = %self.source.app_id, why = %why, "app workspace: store cannot hold one — told the app there is none");
                false
            }
        }
    }

    pub fn handle(&self, mtype: &str, data: &JsValue) {
        match mtype {
            workspace::MSG_LIST => self.list(data),
            workspace::MSG_GET => self.get(data),
            workspace::MSG_SAVE => self.save(data),
            _ => {}
        }
    }

    fn list(&self, data: &JsValue) {
        let id = field(data, "id").unwrap_or(JsValue::NULL);
        let msg = reply(workspace::MSG_LISTING, &id);
        match workspace::list(&self.writer, &self.source.prefix) {
            Ok((files, unreadable)) => {
                set(&msg, "ok", &JsValue::TRUE);
                let arr = js_sys::Array::new();
                for (path, f) in &files {
                    let o = js_sys::Object::new();
                    set(&o, "path", &JsValue::from_str(path));
                    set(&o, "size", &JsValue::from_f64(f.size as f64));
                    set(&o, "mode", &JsValue::from_f64(f.mode as f64));
                    set(&o, "mtime", &JsValue::from_f64(f.mtime as f64));
                    set(&o, "version", &JsValue::from_str(&workspace::version_of(f)));
                    arr.push(&o);
                }
                set(&msg, "files", &arr);
                let bad = js_sys::Array::new();
                for p in &unreadable {
                    bad.push(&JsValue::from_str(p));
                }
                set(&msg, "unreadable", &bad);
                tracing::info!(app = %self.source.app_id, files = files.len(), unreadable = unreadable.len(), "app workspace: listed");
                self.stamp("data-app-work-listed", 1);
            }
            Err(r) => self.refuse(&msg, "list", &r),
        }
        self.post(&msg, None);
    }

    fn get(&self, data: &JsValue) {
        let id = field(data, "id").unwrap_or(JsValue::NULL);
        let msg = reply(workspace::MSG_FILE, &id);
        let path = field(data, "path").and_then(|v| v.as_string()).unwrap_or_default();
        set(&msg, "path", &JsValue::from_str(&path));
        match workspace::read(&self.writer, &self.source.prefix, &path) {
            Ok(bytes) => {
                set(&msg, "ok", &JsValue::TRUE);
                let buf = js_sys::Uint8Array::from(bytes.as_slice()).buffer();
                set(&msg, "data", &buf);
                self.stamp("data-app-work-served", 1);
                self.post(&msg, Some(&buf));
            }
            Err(r) => {
                self.refuse(&msg, &path, &r);
                self.post(&msg, None);
            }
        }
    }

    fn save(&self, data: &JsValue) {
        let id = field(data, "id").unwrap_or(JsValue::NULL);
        let msg = reply(workspace::MSG_SAVED, &id);
        let outcome = parse_save(data).and_then(|(put, remove, expect)| {
            workspace::save_expecting(&self.writer, &self.source.prefix, &put, &remove, &expect)
        });
        match outcome {
            Ok(report) => {
                set(&msg, "ok", &JsValue::from_bool(report.failed.is_empty()));
                set(&msg, "saved", &JsValue::from_f64(report.saved as f64));
                set(&msg, "removed", &JsValue::from_f64(report.removed as f64));
                set(&msg, "bytes", &JsValue::from_f64(report.bytes as f64));
                let failed = js_sys::Array::new();
                for (path, why) in &report.failed {
                    let o = js_sys::Object::new();
                    set(&o, "path", &JsValue::from_str(path));
                    set(&o, "reason", &JsValue::from_str(why.code()));
                    failed.push(&o);
                    tracing::warn!(app = %self.source.app_id, path = %path, reason = why.code(), detail = %why, "app workspace: file not saved");
                }
                set(&msg, "failed", &failed);
                let versions = js_sys::Object::new();
                for (path, version) in &report.versions {
                    set(&versions, path, &JsValue::from_str(version));
                }
                set(&msg, "versions", &versions);
                tracing::info!(
                    app = %self.source.app_id, saved = report.saved, removed = report.removed,
                    bytes = report.bytes, failed = report.failed.len(), "app workspace: saved"
                );
                self.stamp("data-app-work-saves", 1);
                self.stamp("data-app-work-files-saved", report.saved as u32);
            }
            Err(r) => self.refuse(&msg, "save", &r),
        }
        self.post(&msg, None);
    }

    fn refuse(&self, msg: &js_sys::Object, what: &str, r: &Refusal) {
        set(msg, "ok", &JsValue::FALSE);
        set(msg, "reason", &JsValue::from_str(r.code()));
        tracing::warn!(app = %self.source.app_id, what = %what, reason = r.code(), detail = %r, "app workspace: refused");
        let _ = self.frame.set_attribute("data-app-work-last-refusal", r.code());
    }

    fn stamp(&self, attr: &str, by: u32) {
        let n = self
            .frame
            .get_attribute(attr)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            .wrapping_add(by);
        let _ = self.frame.set_attribute(attr, &n.to_string());
    }

    fn post(&self, msg: &js_sys::Object, transfer: Option<&js_sys::ArrayBuffer>) {
        let Some(cw) = self.frame.content_window() else { return };
        if let Some(buf) = transfer {
            let list = js_sys::Array::of1(buf);
            if cw.post_message_with_transfer(msg, "*", &list).is_ok() {
                return;
            }
        }
        let _ = cw.post_message(msg, "*");
    }
}

/// Decode `{put: [{path, mode, mtime, data}], remove: [path]}`. A malformed
/// entry refuses the batch: the app sent something it did not mean, and saving
/// the rest would leave it guessing which half landed.
fn parse_save(data: &JsValue) -> Result<(Vec<PutFile>, Vec<String>, workspace::Expect), Refusal> {
    let bad = |why: &str| Refusal::BadRequest(why.to_string());
    let mut put = Vec::new();
    if let Some(arr) = field(data, "put").filter(|v| !v.is_undefined() && !v.is_null()) {
        let arr: js_sys::Array = arr.dyn_into().map_err(|_| bad("`put` is not an array"))?;
        for item in arr.iter() {
            let path = field(&item, "path").and_then(|v| v.as_string()).ok_or_else(|| bad("a `put` entry has no path"))?;
            let mode = field(&item, "mode").and_then(|v| v.as_f64()).ok_or_else(|| bad("a `put` entry has no mode"))?;
            let mtime = field(&item, "mtime").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let bytes = field(&item, "data")
                .and_then(|v| crate::dom::games::bytes_of(&v))
                .ok_or_else(|| bad("a `put` entry has no data"))?;
            if !(0.0..=u32::MAX as f64).contains(&mode) || mode.fract() != 0.0 {
                return Err(Refusal::BadMode);
            }
            put.push(PutFile {
                path,
                mode: mode as u32,
                mtime: if mtime.is_finite() && mtime > 0.0 { mtime as u64 } else { 0 },
                bytes: bytes.to_vec(),
            });
        }
    }
    let mut remove = Vec::new();
    if let Some(arr) = field(data, "remove").filter(|v| !v.is_undefined() && !v.is_null()) {
        let arr: js_sys::Array = arr.dyn_into().map_err(|_| bad("`remove` is not an array"))?;
        for item in arr.iter() {
            remove.push(item.as_string().ok_or_else(|| bad("a `remove` entry is not a string"))?);
        }
    }
    // `{path: version | null}`; absent means no path is checked.
    let mut expect = workspace::Expect::new();
    if let Some(obj) = field(data, "expect").filter(|v| !v.is_null()) {
        let obj: js_sys::Object = obj.dyn_into().map_err(|_| bad("`expect` is not an object"))?;
        for entry in js_sys::Object::entries(&obj).iter() {
            let pair: js_sys::Array = entry.dyn_into().map_err(|_| bad("`expect` entry"))?;
            let path = pair.get(0).as_string().ok_or_else(|| bad("an `expect` key is not a string"))?;
            let v = pair.get(1);
            let version = if v.is_null() {
                None
            } else {
                Some(v.as_string().ok_or_else(|| bad("an `expect` version is not a string or null"))?)
            };
            expect.insert(path, version);
        }
    }
    Ok((put, remove, expect))
}

use wasm_bindgen::JsCast;

fn field(v: &JsValue, k: &str) -> Option<JsValue> {
    js_sys::Reflect::get(v, &JsValue::from_str(k)).ok().filter(|v| !v.is_undefined())
}

fn set(o: &js_sys::Object, k: &str, v: &JsValue) {
    let _ = js_sys::Reflect::set(o, &JsValue::from_str(k), v);
}

fn reply(mtype: &str, id: &JsValue) -> js_sys::Object {
    let o = js_sys::Object::new();
    set(&o, "source", &JsValue::from_str("entity-host"));
    set(&o, "type", &JsValue::from_str(mtype));
    set(&o, "id", id);
    o
}

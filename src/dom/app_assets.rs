//! The player's half of [`crate::apps::assets`]: answer an app's `x-asset-get`.
//!
//! i18n-ignore-file — every string here is a refusal DETAIL that travels to the
//! app (which branches on the stable `reason` code, never on prose) and to the
//! log. Nothing in this file renders text a person reads.
//!
//! Every decision this makes is in `apps::assets` and gated natively (admission,
//! key validity, index decoding, what is missing, reassembly and its length
//! check). This file only sequences them against a live frame, a writer handle
//! and, when the app came from a publisher's origin, the network.
//!
//! ## One currency check per bundle, before the first answer
//!
//! A bundle's index is the only mutable thing in it (module doc over there). On
//! mount the player asks the origin once whether the held index is current,
//! **and holds requests until that settles** rather than answering from a copy
//! that may be one publish old. Two reasons, and the second decides it:
//!
//! - it is one small request, so the wait is short;
//! - an app reads *related* keys (an index file, then the files it names), and
//!   answering the first from the old index and the rest from the new one would
//!   hand a running program an inconsistent bundle. So a session reads **one**
//!   index, fixed at mount; a republish reaches the next launch.
//!
//! An origin that does not answer leaves the held copy in use (D24's corollary:
//! an outage must not turn into a missing app). No held copy and no answer is
//! `unavailable`, never `not-found` — those are different facts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::JsValue;

use crate::apps::assets::{self, AssetIndex, AssetSource, Refusal};
use crate::content_site::foreign_cache::{self, Currency, ForeignArtifact};
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

enum BundleState {
    /// The currency check is in flight. Requests wait here.
    Settling(Vec<Pending>),
    Ready(Rc<AssetIndex>),
    Failed(Refusal),
}

struct Pending {
    id: JsValue,
    bundle: String,
    key: String,
}

/// Owns the bundle states for one running app. Held by the player's message
/// closure, so it lives exactly as long as the frame does.
pub struct AssetHost {
    source: AssetSource,
    writer: WriterHandle,
    frame: web_sys::HtmlIFrameElement,
    bundles: RefCell<HashMap<String, BundleState>>,
}

impl AssetHost {
    /// Build the host for `source` and start each bundle's currency check.
    /// `None` when the app declares no bundles — it then sees exactly the host
    /// it always had.
    pub fn mount(
        peers: &Peers,
        me: &str,
        source: AssetSource,
        frame: &web_sys::HtmlIFrameElement,
    ) -> Option<Rc<Self>> {
        if source.bundles.is_empty() {
            return None;
        }
        let Some(writer) = peers.writer_handle_for(me) else {
            tracing::warn!(peer = %me, app = %source.app_id, "app assets: no writer handle — every request will be refused");
            return None;
        };
        let host = Rc::new(Self {
            source: source.clone(),
            writer,
            frame: frame.clone(),
            bundles: RefCell::new(HashMap::new()),
        });
        for bundle in &source.bundles {
            let what = ForeignArtifact::AppAssetIndex {
                peer: source.apps_peer.clone(),
                set: source.set.clone(),
                id: source.app_id.clone(),
                bundle: bundle.clone(),
            };
            let held_entity = peers.get_entity(me, &what.store_path());
            let held_index = held_entity.as_ref().and_then(|e| match AssetIndex::from_entity(e) {
                Ok(i) => Some(Rc::new(i)),
                Err(err) => {
                    tracing::warn!(app = %source.app_id, bundle = %bundle, error = %err, "app assets: held index unreadable");
                    None
                }
            });
            match &source.origin {
                None => {
                    let state = match held_index {
                        Some(i) => BundleState::Ready(i),
                        None => BundleState::Failed(Refusal::Unavailable(
                            "no index for this bundle in this profile's tree, and no origin to ask".into(),
                        )),
                    };
                    report_settled(&source, bundle, "local", &state);
                    host.bundles.borrow_mut().insert(bundle.clone(), state);
                }
                Some(origin) => {
                    host.bundles.borrow_mut().insert(bundle.clone(), BundleState::Settling(Vec::new()));
                    let held = foreign_cache::held_hash(peers, me, &what);
                    let host = host.clone();
                    let origin = origin.clone();
                    let bundle = bundle.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let src = crate::content_site::http_poll::FetchBinSource;
                        let outcome = foreign_cache::ensure_current(&src, &host.writer, held, &origin, &what).await;
                        let (state, how) = match outcome {
                            Currency::Fetched(ent) => match AssetIndex::from_entity(&ent) {
                                Ok(i) => (BundleState::Ready(Rc::new(i)), "fetched"),
                                Err(e) => (BundleState::Failed(Refusal::Unreadable(e.to_string())), "fetched-unreadable"),
                            },
                            Currency::Unchanged => match held_index {
                                Some(i) => (BundleState::Ready(i), "current"),
                                None => (
                                    BundleState::Failed(Refusal::Unreadable("the held index is current and unreadable".into())),
                                    "current-unreadable",
                                ),
                            },
                            Currency::Unavailable(e) => match held_index {
                                Some(i) => (BundleState::Ready(i), "origin-silent-held-copy"),
                                None if e.is_terminal() => (BundleState::Failed(Refusal::NotFound), "withheld"),
                                None => (BundleState::Failed(Refusal::Unavailable(e.to_string())), "unreachable"),
                            },
                        };
                        report_settled(&host.source, &bundle, how, &state);
                        host.settle(&bundle, state);
                    });
                }
            }
        }
        Some(host)
    }

    /// The bundle names to advertise in `init`.
    pub fn bundle_names(&self) -> &[String] {
        &self.source.bundles
    }

    /// Handle one `x-asset-get` from the frame. Always answers.
    pub fn handle_get(self: &Rc<Self>, data: &JsValue) {
        let field = |k: &str| js_sys::Reflect::get(data, &JsValue::from_str(k)).ok();
        let id = field("id").unwrap_or(JsValue::NULL);
        let bundle = field("bundle").and_then(|v| v.as_string());
        let key = field("key").and_then(|v| v.as_string());
        if let Err(refusal) = assets::admit(&self.source.bundles, bundle.as_deref(), key.as_deref()) {
            self.reply(&id, bundle.as_deref().unwrap_or(""), key.as_deref().unwrap_or(""), Err(refusal));
            return;
        }
        let (bundle, key) = (bundle.unwrap_or_default(), key.unwrap_or_default());
        let ready = {
            let mut states = self.bundles.borrow_mut();
            match states.get_mut(&bundle) {
                Some(BundleState::Ready(i)) => Ok(i.clone()),
                Some(BundleState::Failed(r)) => Err(Some(r.clone())),
                Some(BundleState::Settling(waiting)) => {
                    waiting.push(Pending { id: id.clone(), bundle: bundle.clone(), key: key.clone() });
                    Err(None)
                }
                // `admit` passed, so the bundle is declared, and `mount` made a
                // state for every declared bundle.
                None => Err(Some(Refusal::NotDeclared)),
            }
        };
        match ready {
            Ok(index) => self.serve(index, id, bundle, key),
            Err(Some(refusal)) => self.reply(&id, &bundle, &key, Err(refusal)),
            Err(None) => {}
        }
    }

    fn settle(self: &Rc<Self>, bundle: &str, state: BundleState) {
        let waiting = {
            let mut states = self.bundles.borrow_mut();
            let prev = states.insert(bundle.to_string(), state);
            match prev {
                Some(BundleState::Settling(w)) => w,
                _ => Vec::new(),
            }
        };
        for p in waiting {
            self.handle_pending(p);
        }
    }

    fn handle_pending(self: &Rc<Self>, p: Pending) {
        let state = match self.bundles.borrow().get(&p.bundle) {
            Some(BundleState::Ready(i)) => Ok(i.clone()),
            Some(BundleState::Failed(r)) => Err(r.clone()),
            _ => Err(Refusal::Unavailable("bundle did not settle".into())),
        };
        match state {
            Ok(index) => self.serve(index, p.id, p.bundle, p.key),
            Err(r) => self.reply(&p.id, &p.bundle, &p.key, Err(r)),
        }
    }

    fn serve(self: &Rc<Self>, index: Rc<AssetIndex>, id: JsValue, bundle: String, key: String) {
        let Some(entry) = index.entries.get(&key).copied() else {
            self.reply(&id, &bundle, &key, Err(Refusal::NotFound));
            return;
        };
        let missing = assets::missing_content(&entry, |h| self.writer.content_get(h));
        match (missing, &self.source.origin) {
            (Err(r), _) => self.reply(&id, &bundle, &key, Err(r)),
            (Ok(m), _) if m.is_empty() => {
                let out = assets::resolve_entry(&entry, |h| self.writer.content_get(h));
                self.reply(&id, &bundle, &key, out);
            }
            (Ok(_), None) => self.reply(
                &id,
                &bundle,
                &key,
                Err(Refusal::Unavailable("the file's bytes are not held and there is no origin to ask".into())),
            ),
            (Ok(_), Some(origin)) => {
                let host = self.clone();
                let origin = origin.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let src = crate::content_site::http_poll::FetchBinSource;
                    let out = match foreign_cache::ensure_content(&src, &host.writer, &origin, &entry.blob).await {
                        Ok(()) => assets::resolve_entry(&entry, |h| host.writer.content_get(h)),
                        Err(e) => Err(Refusal::Unavailable(e.to_string())),
                    };
                    host.reply(&id, &bundle, &key, out);
                });
            }
        }
    }

    /// Post the answer, and stamp the frame so a gate can count outcomes
    /// without reading into an opaque-origin document.
    fn reply(&self, id: &JsValue, bundle: &str, key: &str, outcome: Result<Vec<u8>, Refusal>) {
        let msg = js_sys::Object::new();
        let set = |k: &str, v: &JsValue| {
            let _ = js_sys::Reflect::set(&msg, &JsValue::from_str(k), v);
        };
        set("source", &JsValue::from_str("entity-host"));
        set("type", &JsValue::from_str(assets::MSG_ASSET));
        set("id", id);
        set("bundle", &JsValue::from_str(bundle));
        set("key", &JsValue::from_str(key));
        set("ok", &JsValue::from_bool(outcome.is_ok()));
        let transfer = js_sys::Array::new();
        let stamp = match &outcome {
            Ok(bytes) => {
                let arr = js_sys::Uint8Array::from(bytes.as_slice());
                let buf = arr.buffer();
                set("size", &JsValue::from_f64(bytes.len() as f64));
                set("data", &buf);
                transfer.push(&buf);
                "data-app-assets-served"
            }
            Err(r) => {
                set("reason", &JsValue::from_str(r.code()));
                tracing::warn!(app = %self.source.app_id, bundle = %bundle, key = %key, reason = r.code(), detail = %r, "app asset: refused");
                let _ = self.frame.set_attribute("data-app-assets-last-refusal", r.code());
                "data-app-assets-refused"
            }
        };
        let n = self
            .frame
            .get_attribute(stamp)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            .wrapping_add(1);
        let _ = self.frame.set_attribute(stamp, &n.to_string());
        if let Some(cw) = self.frame.content_window() {
            if cw.post_message_with_transfer(&msg, "*", &transfer).is_err() {
                let _ = cw.post_message(&msg, "*");
            }
        }
    }
}

/// One line per bundle per mount, from every arm — a bundle that settled
/// successfully must be as visible as one that did not (D13).
fn report_settled(source: &AssetSource, bundle: &str, how: &str, state: &BundleState) {
    match state {
        BundleState::Ready(i) => tracing::info!(
            app = %source.app_id, bundle = %bundle, how = %how, files = i.entries.len(),
            bytes = i.total_bytes(), "app assets: bundle ready"
        ),
        BundleState::Failed(r) => tracing::warn!(
            app = %source.app_id, bundle = %bundle, how = %how, reason = r.code(), detail = %r,
            "app assets: bundle unavailable"
        ),
        BundleState::Settling(_) => {}
    }
}

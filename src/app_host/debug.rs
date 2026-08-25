//! app_host debug overlay — a developer-facing panel that makes the running
//! compute program's mount-contract wiring and live tree state visible, plus
//! a single-step control for the paused clock. This is a LOCAL diagnostic
//! surface only: it reads the inner peer directly (the same `&Peers` the tick
//! loop already holds) and never crosses ③α — the outer host still only ever
//! sees `state` emissions (P1 untouched).
//!
//! v2: one merged "wiring" view instead of a static topology `<pre>` +
//! a separately-refreshed tree-dump `<pre>` you had to cross-reference by
//! path — a row per wired thing (state, each output port, each input port),
//! each showing its relationship (step / source / seed) AND its live decoded
//! value together, with a brief flash when a value's content hash changes
//! tick-to-tick (so you can actually watch data move, not just re-read a
//! static dump). The untouched static bundle (IR/code/assets materialized
//! once at F-E1) collapses into a `<details>` summary instead of padding out
//! the live rows.
//! - **Wiring rows**: built once from the already-decoded [`ProgramDescriptor`]
//!   (free — no eval) and never re-created; only each row's value `<pre>` is
//!   patched in place on refresh.
//! - **Change detection** reuses the store's own content hash
//!   ([`entity_store::LocationEntry::hash`], via [`crate::peers::Peers`]'s
//!   `tree_listing`) instead of re-hashing the decoded preview — exact and
//!   free (the tree_listing walk already computed it).

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::dom::util;
use crate::peers::Peers;
use crate::program_host::descriptor::ProgramDescriptor;
use crate::program_host::host;

/// Debug-panel styling, appended alongside [`super::onscreen::CONTROLS_CSS`].
/// `.ah-debug` is hidden by default (`data-mode="hidden"`, same show/hide
/// convention as the on-screen pad). ONE scroll region on the panel itself
/// (not per-row) — on a small/fixed-size window (Tauri's default 1280x720
/// with no page scroll) a panel with no height cap could balloon the page
/// past the viewport with no way to reach the rest of it; capping the
/// panel's own height and letting `[data-app-host]` (see CONTROLS_CSS) scroll
/// the page is the one scroll boundary that matters.
///
/// The flash highlight uses TWO identical keyframes (`ah-flash-a`/`-b`)
/// rather than one: re-adding the SAME animation-name to an element that
/// already has it doesn't restart it (no computed-style change to react to),
/// so each content change alternates between the two names — a cheap,
/// timer-free "did this just change" pulse.
pub const DEBUG_CSS: &str = "\
.ah-debug{max-width:420px;margin:8px auto 0;max-height:280px;overflow-y:auto;\
border:1px solid var(--border, #333);border-radius:8px;padding:8px;\
background:var(--surface-sunken, #0a0a1a);}\
.ah-debug[data-mode=\"hidden\"]{display:none;}\
.ah-debug h4{margin:0 0 4px;font-size:12px;font-weight:600;color:var(--text-muted, #c0c0c0);}\
.ah-wire-summary{font-size:10px;color:var(--text-dim, #888);margin:0 0 6px;}\
.ah-wiring{display:flex;flex-direction:column;gap:6px;}\
.ah-wire-row{border:1px solid var(--border, #333);border-radius:6px;padding:6px;}\
.ah-wire-top{display:flex;align-items:center;gap:6px;flex-wrap:wrap;}\
.ah-wire-role{font-size:9px;font-weight:700;letter-spacing:.04em;padding:1px 5px;\
border-radius:999px;color:var(--accent-text, #1a1a2e);background:var(--text-muted, #c0c0c0);}\
.ah-wire-role[data-role=\"state\"]{background:var(--accent, #90d0ff);}\
.ah-wire-role[data-role=\"out\"]{background:var(--accent-green, #c0e0c0);}\
.ah-wire-role[data-role=\"in\"]{background:var(--accent-2, #c0c0e0);}\
.ah-wire-name{font-size:12px;font-weight:600;color:var(--text, #e0e0e0);}\
.ah-wire-path{font-size:10px;font-family:ui-monospace,monospace;color:var(--text-dim, #888);\
margin:2px 0;word-break:break-all;}\
.ah-wire-rel{font-size:10px;font-family:ui-monospace,monospace;color:var(--text-dim, #888);\
margin:0 0 4px;word-break:break-all;}\
.ah-wire-expr, .ah-wire-value{display:block;margin:0 0 4px;}\
.ah-wire-expr summary, .ah-wire-value summary{cursor:pointer;font-size:10px;\
font-family:ui-monospace,monospace;color:var(--text-dim, #888);border-radius:4px;padding:1px 3px;}\
.ah-wire-expr pre, .ah-wire-value pre{margin:2px 0 0;white-space:pre-wrap;word-break:break-all;\
font-family:ui-monospace,monospace;font-size:11px;line-height:1.4;color:var(--text, #e0e0e0);\
padding:2px 4px;}\
[data-flash=\"a\"]{animation:ah-flash-a 700ms ease-out;}\
[data-flash=\"b\"]{animation:ah-flash-b 700ms ease-out;}\
@keyframes ah-flash-a{from{background:var(--accent, rgba(144,208,255,0.35));}to{background:transparent;}}\
@keyframes ah-flash-b{from{background:var(--accent, rgba(144,208,255,0.35));}to{background:transparent;}}\
.ah-wire-bundle{margin-top:4px;font-size:10px;color:var(--text-dim, #888);}\
.ah-wire-bundle summary{cursor:pointer;}\
.ah-wire-bundle pre{margin:4px 0 0;white-space:pre-wrap;word-break:break-all;\
font-family:ui-monospace,monospace;font-size:10px;line-height:1.4;color:var(--text-dim, #888);}";

/// One wired thing's live slot — the path to poll and the DOM handles
/// [`refresh`] patches in place. Built once by [`build`]; never recreated.
struct WiringRow {
    /// Namespace-qualified path (the `get_entity`/`tree_listing` key).
    path: String,
    /// The value `<details>`'s `<summary>` — kept visible even while the
    /// body is collapsed, so a flash (and the type/size caption) is still
    /// noticeable without expanding a big row.
    value_summary_el: Element,
    value_el: Element,
    /// The store's own content hash for this path, last seen — an exact,
    /// free change signal (no re-hashing the decoded preview).
    last_hash: RefCell<String>,
    /// Alternates so the flash keyframe restarts every change (see
    /// [`DEBUG_CSS`] doc comment).
    flip: Cell<bool>,
}

/// What [`build`] hands back: the toggle chip (mount into the chrome bar),
/// the panel (mount below the status caption), the per-path wiring rows the
/// tick loop refreshes, and the `shown` flag gating that refresh. The click
/// `Closure` is returned SEPARATELY (not a field here) so this struct stays
/// a plain value the caller can move as a whole into the tick-loop future —
/// once the closure is registered into `LIVE` (document lifetime, D12 —
/// never `Closure::forget`), nothing else needs to touch it.
pub struct DebugPanel {
    pub chip: Element,
    pub panel: Element,
    summary: Element,
    wiring: Vec<WiringRow>,
    bundle_summary: Element,
    bundle_pre: Element,
    /// Whether the panel is currently shown — the tick loop only recomputes
    /// the wiring values (a `tree_listing` + N `get_entity` reads) when this
    /// is true, so a hidden panel costs nothing per tick.
    pub shown: Rc<Cell<bool>>,
}

/// Append one wiring row: role badge, name, path, relationship line, an
/// OPTIONAL collapsed `expr` block (the decoded compute-expression node the
/// relationship points at — `step`/`source`/`seed`, static, rendered once),
/// and a collapsible value block (live, [`refresh`] patches its `<summary>`
/// + `<pre>` in place). `value_open` controls whether the value starts
/// expanded — ports default open (usually small), STATE defaults closed
/// (can be the biggest thing on the panel by far, e.g. Life's whole board).
/// Returns `(value_summary_el, value_el)` for [`WiringRow`] to hold.
fn build_row(
    parent: &Element,
    role: &str,
    path: &str,
    name_line: &str,
    rel_line: &str,
    expr: Option<(&str, &str)>,
    value_open: bool,
) -> (Element, Element) {
    let row = util::create_element("div");
    util::set_attr(&row, "class", "ah-wire-row");

    let top = util::create_element("div");
    util::set_attr(&top, "class", "ah-wire-top");
    let role_el = util::create_element("span");
    util::set_attr(&role_el, "class", "ah-wire-role");
    util::set_attr(&role_el, "data-role", &role.to_lowercase());
    util::set_text(&role_el, role);
    util::append(&top, &role_el);
    let name_el = util::create_element("span");
    util::set_attr(&name_el, "class", "ah-wire-name");
    util::set_text(&name_el, name_line);
    util::append(&top, &name_el);
    util::append(&row, &top);

    let path_el = util::create_element("div");
    util::set_attr(&path_el, "class", "ah-wire-path");
    util::set_text(&path_el, path);
    util::append(&row, &path_el);

    let rel_el = util::create_element("div");
    util::set_attr(&rel_el, "class", "ah-wire-rel");
    util::set_text(&rel_el, rel_line);
    util::append(&row, &rel_el);

    if let Some((expr_type, expr_body)) = expr {
        let details = util::create_element("details");
        util::set_attr(&details, "class", "ah-wire-expr");
        let sum = util::create_element("summary");
        util::set_text(&sum, &format!("expr: {expr_type}"));
        util::append(&details, &sum);
        let pre = util::create_element("pre");
        util::set_text(&pre, expr_body);
        util::append(&details, &pre);
        util::append(&row, &details);
    }

    let value_details = util::create_element("details");
    util::set_attr(&value_details, "class", "ah-wire-value");
    if value_open {
        util::set_attr(&value_details, "open", "");
    }
    let value_summary_el = util::create_element("summary");
    util::set_text(&value_summary_el, "value \u{2014} pending first tick");
    util::append(&value_details, &value_summary_el);
    let value_el = util::create_element("pre");
    util::set_attr(&value_el, "class", "ah-wire-val");
    util::set_text(&value_el, "\u{2026}"); // … — pending first refresh
    util::append(&value_details, &value_el);
    util::append(&row, &value_details);

    util::append(parent, &row);
    (value_summary_el, value_el)
}

// --- Compute-expression s-expression rendering -----------------------------
//
// A `step` / `source` / seed reference is a PATH to a `compute/*` expression
// node (`program_host::host::eval_future`'s `system/compute:eval` contract).
// That node's own fields reference CHILD expression nodes not by path but by
// CONTENT HASH (a 33-byte `system/hash` value: 1 format byte + a 32-byte
// SHA-256 digest of the child entity's ECF encoding — EXTENSION-COMPUTE §2,
// §3) — the compute IR is a content-addressed DAG, not a tree of paths. To
// render the WHOLE expression (not just its root node), a hash-ref has to be
// resolved to the entity it names and recursively rendered in turn.
//
// [`build_hash_index`] does that resolution the only way available here (no
// hash-indexed store API on `Peers` — see `src/peers.rs`): walk the whole
// program namespace once via `tree_listing` (already the same call
// [`refresh`] makes for the static-bundle listing) and index every entity's
// own content hash → its path, so a hash-ref found inside a decoded node can
// be looked up and re-fetched by path. Built ONCE per panel (static — the
// expression graph never changes at runtime), never touched again.

/// Every entity's content hash (hex digest, no format byte, no `ecf[v1]-` tag
/// — `EXTENSION-COMPUTE` §3 confirms only the digest matters, the tag is
/// cosmetic) under the program's namespace, mapped to its path — a hash-ref
/// resolver built from what `Peers` actually exposes (path-keyed reads).
fn build_hash_index(peers: &Peers, peer_id: &str, ns: &str) -> BTreeMap<String, String> {
    peers
        .tree_listing(peer_id, &format!("/{ns}/"))
        .into_iter()
        .map(|e| {
            let full = e.hash.to_string();
            let hex = full.rsplit(':').next().unwrap_or(&full).to_string();
            (hex, e.path)
        })
        .collect()
}

fn cbor_text(v: &ciborium::Value) -> Option<&str> {
    match v {
        ciborium::Value::Text(t) => Some(t.as_str()),
        _ => None,
    }
}
fn cbor_bytes(v: &ciborium::Value) -> Option<&[u8]> {
    match v {
        ciborium::Value::Bytes(b) => Some(b.as_slice()),
        _ => None,
    }
}
fn cbor_array(v: &ciborium::Value) -> Option<&[ciborium::Value]> {
    match v {
        ciborium::Value::Array(a) => Some(a.as_slice()),
        _ => None,
    }
}
fn cbor_map(v: &ciborium::Value) -> Option<&[(ciborium::Value, ciborium::Value)]> {
    match v {
        ciborium::Value::Map(m) => Some(m.as_slice()),
        _ => None,
    }
}
fn map_get<'a>(map: &'a [(ciborium::Value, ciborium::Value)], key: &str) -> Option<&'a ciborium::Value> {
    map.iter().find(|(k, _)| cbor_text(k) == Some(key)).map(|(_, v)| v)
}

/// Node-visit budget for one root expression's render — a cheap circuit
/// breaker against a pathological/self-referential graph, independent of
/// [`MAX_EXPR_DEPTH`] (a wide-but-shallow DAG with heavily shared
/// sub-expressions could otherwise blow up the output without ever hitting
/// the depth cap, since a shared node is re-rendered at every place it's
/// referenced — a Lisp-style flattened expansion, same as writing it out by
/// hand). Generous for any real authored program; not reachable in practice.
const MAX_EXPR_NODES: usize = 800;
const MAX_EXPR_DEPTH: usize = 40;

/// Render one decoded compute-expression node — and everything its hash-refs
/// point at, recursively — as a Lisp-style s-expression. Field names/shapes
/// per node type are `EXTENSION-COMPUTE` §2.1–2.2 (mirrored 1:1 in
/// `entity-core-rust/extensions/compute/src/eval/*.rs`, the evaluator that
/// is the actual runtime authority): `(op left right)` for
/// arithmetic/compare/logic (the `op` field IS the natural Lisp head symbol),
/// `(let ((name val) …) body)`, `(if cond then else)`, `(lambda (params…)
/// body)`, `(tree-ref "path")` / a bare `name` for the two lookup forms,
/// `(call "path" :op (arg val) …)` / `(apply fn (arg val) …)` for the two
/// `compute/apply` modes. Anything not in that list (a value type like
/// `compute/result`, or a genuinely unknown type) falls back to
/// `({type} {single-level pretty fields})` rather than vanishing — D13.
fn sexpr(
    peers: &Peers,
    peer_id: &str,
    by_hash: &BTreeMap<String, String>,
    budget: &Cell<usize>,
    entity_type: &str,
    data: &[u8],
    depth: usize,
) -> String {
    if depth > MAX_EXPR_DEPTH {
        return "\u{2026}(depth limit)".to_string();
    }
    if budget.get() == 0 {
        return "\u{2026}(expr too large)".to_string();
    }
    budget.set(budget.get() - 1);

    let Ok(v) = ciborium::from_reader::<ciborium::Value, _>(data) else {
        return format!("<{} raw bytes, not CBOR>", data.len());
    };
    let Some(map) = cbor_map(&v) else {
        return pretty_cbor(&v, depth);
    };

    let resolve = |field: &ciborium::Value| -> String {
        match cbor_bytes(field) {
            Some(bytes) if bytes.len() > 1 => {
                let hex: String = bytes[1..].iter().map(|b| format!("{b:02x}")).collect();
                match by_hash.get(&hex).and_then(|p| peers.get_entity(peer_id, p)) {
                    Some(entity) => sexpr(peers, peer_id, by_hash, budget, &entity.entity_type, &entity.data, depth + 1),
                    None => format!("#unresolved:{}", hex.get(..10).unwrap_or(&hex)),
                }
            }
            _ => pretty_cbor(field, depth), // an inline (non-ref) field value
        }
    };
    let pad = "  ".repeat(depth + 1);
    let field = |key: &str| map_get(map, key);
    let field_str = |key: &str| field(key).map(resolve).unwrap_or_else(|| "?".to_string());
    let field_text = |key: &str| field(key).and_then(cbor_text).unwrap_or("?");

    match entity_type {
        "compute/literal" => field("value").map(|v| pretty_cbor(v, depth)).unwrap_or_default(),
        "compute/lookup/scope" => field_text("name").to_string(),
        "compute/lookup/tree" => {
            let path = field_text("path");
            if field("relative").is_some_and(|v| matches!(v, ciborium::Value::Bool(true))) {
                format!("(tree-ref {path:?} :relative)")
            } else {
                format!("(tree-ref {path:?})")
            }
        }
        "compute/lookup/hash" => match field("hash") {
            Some(h) => format!("(deref {})", resolve(h)),
            None => "(deref ?)".to_string(),
        },
        "compute/if" => {
            let (c, t) = (field_str("condition"), field_str("then"));
            match field("else") {
                Some(e) => format!("(if {c}\n{pad}{t}\n{pad}{})", resolve(e)),
                None => format!("(if {c}\n{pad}{t})"),
            }
        }
        "compute/let" => {
            let mut bindings = String::new();
            for entry in field("bindings").and_then(cbor_array).unwrap_or(&[]) {
                let Some(bm) = cbor_map(entry) else { continue };
                let name = map_get(bm, "name").and_then(cbor_text).unwrap_or("?");
                let val = map_get(bm, "value").map(resolve).unwrap_or_else(|| "?".to_string());
                bindings.push_str(&format!("\n{pad}  ({name} {val})"));
            }
            format!("(let ({bindings}\n{pad})\n{pad}{})", field_str("body"))
        }
        "compute/lambda" => {
            let params: Vec<&str> = field("params")
                .and_then(cbor_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(cbor_text)
                .collect();
            format!("(lambda ({}) {})", params.join(" "), field_str("body"))
        }
        "compute/arithmetic" | "compute/compare" | "compute/logic" => {
            let op = field_text("op");
            match field("right") {
                Some(_) => format!("({op} {} {})", field_str("left"), field_str("right")),
                None => format!("({op} {})", field_str("left")), // `logic:not` — unary
            }
        }
        "compute/field" => format!("(field {} {:?})", field_str("entity"), field_text("name")),
        "compute/construct" => {
            let mut fields = String::new();
            for (k, val) in field("fields").and_then(cbor_map).unwrap_or(&[]) {
                fields.push_str(&format!("\n{pad}  ({} {})", cbor_text(k).unwrap_or("?"), resolve(val)));
            }
            format!("(construct {:?}{fields}\n{pad})", field_text("entity_type"))
        }
        "compute/index" => format!("(index {} {})", field_str("array"), field_str("index")),
        "compute/length" => format!("(length {})", field_str("array")),
        "compute/numeric-cast" => format!("(cast {} {:?})", field_str("value"), field_text("to_type")),
        "compute/apply" => {
            let head = if let Some(path) = field("path").and_then(cbor_text) {
                format!("(call {path:?} :{}", field_text("operation"))
            } else if let Some(f) = field("fn") {
                format!("(apply {}", resolve(f))
            } else {
                "(apply ?".to_string()
            };
            let mut args = String::new();
            for (k, val) in field("args").and_then(cbor_map).unwrap_or(&[]) {
                args.push_str(&format!("\n{pad}  ({} {})", cbor_text(k).unwrap_or("?"), resolve(val)));
            }
            format!("{head}{args}\n{pad})")
        }
        other => format!("({other} {})", pretty_cbor(&v, depth)),
    }
}

/// Resolve `path` (a `step`/`source`/seed reference) to its entity and render
/// it — recursively, via [`sexpr`] — as a Lisp-style s-expression. Static
/// (expressions don't change at runtime), so [`build`] calls this once per
/// row and never again; `by_hash` (from [`build_hash_index`]) is built once
/// for the whole panel and threaded through every row's call.
fn expr_sexpr(peers: &Peers, peer_id: &str, by_hash: &BTreeMap<String, String>, path: &str) -> (String, String) {
    match peers.get_entity(peer_id, path) {
        Some(entity) => {
            let budget = Cell::new(MAX_EXPR_NODES);
            let body = sexpr(peers, peer_id, by_hash, &budget, &entity.entity_type, &entity.data, 0);
            (entity.entity_type.clone(), body)
        }
        None => ("?".to_string(), format!("<unresolved: {path}>")),
    }
}

/// Build the debug chip + panel. Rows are laid out in mount-contract order
/// (state, then each output port, then each input port) straight from `desc`
/// — the wiring never changes at runtime, only the values flowing through it
/// do, so only [`refresh`] touches anything after this. `peers`/`peer_id`
/// are read ONCE here too, to decode each row's `step`/`source`/`seed`
/// compute-expression node — safe because materialize (F-E1) has already
/// landed the whole static bundle by the time this runs.
pub fn build(
    peers: &Peers,
    peer_id: &str,
    desc: &ProgramDescriptor,
    ns: &str,
) -> (DebugPanel, Vec<Closure<dyn FnMut(JsValue)>>) {
    let chip = util::create_element("button");
    util::set_attr(&chip, "type", "button");
    util::set_attr(&chip, "class", "ah-chip");
    util::set_attr(&chip, "data-debug-toggle", "");
    util::set_attr(&chip, "aria-label", "debug panel");
    util::set_text(&chip, "\u{1F41E}"); // 🐞

    let panel = util::create_element("div");
    util::set_attr(&panel, "class", "ah-debug");
    util::set_attr(&panel, "data-mode", "hidden");
    util::set_attr(&panel, "data-app-host-debug", "");

    let heading = util::create_element("h4");
    util::set_text(&heading, "Compute wiring (live)");
    util::append(&panel, &heading);

    let prefix = format!("/{ns}/");
    let summary = util::create_element("div");
    util::set_attr(&summary, "class", "ah-wire-summary");
    util::set_text(&summary, &format!("entities under {prefix} — waiting for first tick"));
    util::append(&panel, &summary);

    let wiring_el = util::create_element("div");
    util::set_attr(&wiring_el, "class", "ah-wiring");
    util::set_attr(&wiring_el, "data-app-host-debug-wiring", "");
    util::append(&panel, &wiring_el);

    let mut wiring = Vec::new();
    // Built once, threaded through every row's `expr_sexpr` call below — see
    // the "Compute-expression s-expression rendering" module comment.
    let by_hash = build_hash_index(peers, peer_id, ns);

    let state_path = host::qualify(ns, &desc.state_path);
    let (step_type, step_body) = expr_sexpr(peers, peer_id, &by_hash, &host::qualify(ns, &desc.step));
    let (value_summary_el, value_el) = build_row(
        &wiring_el,
        "STATE",
        &state_path,
        &desc.state_path,
        &format!(
            "step: {}  ({} @ {}Hz)",
            desc.step, desc.tick.mode, desc.tick.rate_hint
        ),
        Some((&step_type, &step_body)),
        false, // STATE starts collapsed — often the biggest value on the panel
    );
    wiring.push(WiringRow {
        path: state_path,
        value_summary_el,
        value_el,
        last_hash: RefCell::new(String::new()),
        flip: Cell::new(false),
    });

    for p in &desc.output_ports {
        let path = host::qualify(ns, &p.path);
        let (rel, expr) = match &p.source {
            Some(src) => {
                let (ty, body) = expr_sexpr(peers, peer_id, &by_hash, &host::qualify(ns, src));
                (format!("source: {src}"), Some((ty, body)))
            }
            None => ("(written by step)".to_string(), None),
        };
        let (value_summary_el, value_el) = build_row(
            &wiring_el,
            "OUT",
            &path,
            &format!("{}  [{}]", p.name, p.shape),
            &rel,
            expr.as_ref().map(|(ty, body)| (ty.as_str(), body.as_str())),
            true,
        );
        wiring.push(WiringRow {
            path,
            value_summary_el,
            value_el,
            last_hash: RefCell::new(String::new()),
            flip: Cell::new(false),
        });
    }
    for p in &desc.input_ports {
        let path = host::qualify(ns, &p.path);
        let seed = p.initial.as_deref().unwrap_or("?");
        let rel = format!("seed: {seed}");
        let expr = p
            .initial
            .as_deref()
            .map(|seed| expr_sexpr(peers, peer_id, &by_hash, &host::qualify(ns, seed)));
        let (value_summary_el, value_el) = build_row(
            &wiring_el,
            "IN",
            &path,
            &format!("{}  [{}]", p.name, p.shape),
            &rel,
            expr.as_ref().map(|(ty, body)| (ty.as_str(), body.as_str())),
            true,
        );
        wiring.push(WiringRow {
            path,
            value_summary_el,
            value_el,
            last_hash: RefCell::new(String::new()),
            flip: Cell::new(false),
        });
    }

    let bundle = util::create_element("details");
    util::set_attr(&bundle, "class", "ah-wire-bundle");
    let bundle_summary = util::create_element("summary");
    // Same "pending" convention as `summary` and each row's value block
    // (`\u{2026}`/"waiting for first tick") — without it, opening the panel
    // before the first refresh (e.g. while paused) makes this section look
    // broken/empty rather than just not-yet-populated.
    util::set_text(&bundle_summary, "static bundle \u{2014} waiting for first tick");
    util::append(&bundle, &bundle_summary);
    let bundle_pre = util::create_element("pre");
    util::set_text(&bundle_pre, "\u{2026}");
    util::append(&bundle, &bundle_pre);
    util::append(&panel, &bundle);

    let shown = Rc::new(Cell::new(false));
    let cb = {
        let shown = shown.clone();
        let panel = panel.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
                ev.prevent_default();
            }
            let next = !shown.get();
            shown.set(next);
            let _ = panel.set_attribute("data-mode", if next { "shown" } else { "hidden" });
        }) as Box<dyn FnMut(JsValue)>)
    };
    let _ = chip.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());

    (
        DebugPanel {
            chip,
            panel,
            summary,
            wiring,
            bundle_summary,
            bundle_pre,
            shown,
        },
        vec![cb],
    )
}

fn is_cbor_scalar(v: &ciborium::Value) -> bool {
    !matches!(v, ciborium::Value::Array(_) | ciborium::Value::Map(_))
}

/// A pretty, indented rendering of a decoded CBOR value — JSON-ish, not the
/// single-line `Map([(Text("x"), Integer(3))])` `Debug` dump, which reads as
/// a wall of Rust syntax rather than a value you can eyeball at a glance.
/// Best-effort: no per-type inspector, no click-to-expand (`app_host` has no
/// `DomCtx` to wire that with) — just legible enough to watch a number or a
/// small struct change tick to tick.
fn pretty_cbor(v: &ciborium::Value, depth: usize) -> String {
    use ciborium::Value as V;
    let pad = "  ".repeat(depth + 1);
    let close = "  ".repeat(depth);
    match v {
        V::Map(m) if !m.is_empty() => {
            let mut s = String::from("{\n");
            for (k, val) in m {
                let key = match k {
                    V::Text(t) => t.clone(),
                    other => format!("{other:?}"),
                };
                s.push_str(&format!("{pad}{key}: {}\n", pretty_cbor(val, depth + 1)));
            }
            s.push_str(&format!("{close}}}"));
            s
        }
        V::Map(_) => "{}".to_string(),
        // Capped, and INLINE (one line, comma-separated) when every element
        // is a scalar — the common case for a literal data array (e.g. a
        // display expression's coordinate ramps, which run to hundreds of
        // entries: one-line-per-number there would bury everything else on
        // the panel). Nested arrays/maps keep the one-per-line form below,
        // where a line break earns its keep.
        V::Array(a) if !a.is_empty() && a.iter().all(is_cbor_scalar) => {
            const MAX_ITEMS: usize = 16;
            let show = a.len().min(MAX_ITEMS);
            let mut items: Vec<String> = a[..show].iter().map(|x| pretty_cbor(x, depth)).collect();
            if a.len() > show {
                items.push(format!("\u{2026} ({} more)", a.len() - show));
            }
            format!("[{}]", items.join(", "))
        }
        V::Array(a) if !a.is_empty() => {
            const MAX_ITEMS: usize = 16;
            let show = a.len().min(MAX_ITEMS);
            let mut s = String::from("[\n");
            for val in &a[..show] {
                s.push_str(&format!("{pad}{}\n", pretty_cbor(val, depth + 1)));
            }
            if a.len() > show {
                s.push_str(&format!("{pad}\u{2026} ({} more)\n", a.len() - show));
            }
            s.push_str(&format!("{close}]"));
            s
        }
        V::Array(_) => "[]".to_string(),
        V::Text(t) => format!("{t:?}"),
        // Hex, not just a length — expression-IR nodes reference child nodes
        // by content hash (`_expr/<hex>` sibling paths in the static bundle);
        // [`sexpr`] resolves and recurses into those, but any byte value NOT
        // resolved that way (e.g. inside an opaque literal) at least shows a
        // hash you can go cross-reference by hand.
        V::Bytes(b) => {
            let hex: String = b.iter().take(32).map(|byte| format!("{byte:02x}")).collect();
            if b.len() > 32 {
                format!("0x{hex}\u{2026} ({} bytes)", b.len())
            } else {
                format!("0x{hex} ({} bytes)", b.len())
            }
        }
        V::Bool(b) => b.to_string(),
        V::Null => "null".to_string(),
        V::Integer(i) => i128::from(*i).to_string(),
        V::Float(f) => format!("{f}"),
        V::Tag(_, inner) => pretty_cbor(inner, depth),
        other => format!("{other:?}"),
    }
}

/// Best-effort decode + pretty-print of one entity's raw data, truncated by
/// CHAR count (not byte count — the pretty text can carry multi-byte UTF-8
/// from string values, so a byte slice could split mid-character).
fn decode_preview(data: &[u8]) -> String {
    const MAX_CHARS: usize = 500;
    match ciborium::from_reader::<ciborium::Value, _>(data) {
        Ok(v) => {
            let s = pretty_cbor(&v, 0);
            if s.chars().count() > MAX_CHARS {
                let truncated: String = s.chars().take(MAX_CHARS).collect();
                format!("{truncated}\u{2026}")
            } else {
                s
            }
        }
        Err(_) => format!("<{} raw bytes, not CBOR>", data.len()),
    }
}

/// Refresh every wiring row's live value (only the ones whose content hash
/// actually changed get touched + flashed) plus the summary line and the
/// static-bundle `<details>`. Direct-arm only (`app_host` always runs
/// `Peers::new_direct()`), so `tree_listing`/`get_entity` are synchronous,
/// cheap, in-memory reads; call this only while the panel is visible
/// ([`DebugPanel::shown`]) to avoid paying it every tick for nothing.
pub fn refresh(panel: &DebugPanel, peers: &Peers, peer_id: &str, ns: &str) {
    let prefix = format!("/{ns}/");
    let mut entries = peers.tree_listing(peer_id, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let by_path: BTreeMap<_, _> = entries.iter().map(|e| (e.path.as_str(), e)).collect();

    for row in &panel.wiring {
        let Some(entry) = by_path.get(row.path.as_str()) else {
            util::set_text(&row.value_summary_el, "value \u{2014} absent");
            util::set_text(&row.value_el, "<absent>");
            continue;
        };
        let hash = entry.hash.to_string();
        if *row.last_hash.borrow() == hash {
            continue; // unchanged since last refresh — leave it alone (no flash)
        }
        *row.last_hash.borrow_mut() = hash.clone();
        match peers.get_entity(peer_id, &row.path) {
            Some(entity) => {
                let short_hash = hash.get(..12).unwrap_or(&hash);
                // The caption goes on the SUMMARY — always visible even while
                // the body is collapsed — so a flash there is noticeable
                // without expanding a big row (e.g. STATE, closed by default).
                util::set_text(
                    &row.value_summary_el,
                    &format!("value \u{2014} {}  [{}B, {short_hash}]", entity.entity_type, entity.data.len()),
                );
                util::set_text(&row.value_el, &decode_preview(&entity.data));
            }
            None => {
                util::set_text(&row.value_summary_el, "value \u{2014} unreadable");
                util::set_text(&row.value_el, "<unreadable>");
            }
        }
        let flip = !row.flip.get();
        row.flip.set(flip);
        let flash = if flip { "a" } else { "b" };
        let _ = row.value_summary_el.set_attribute("data-flash", flash);
    }

    util::set_text(&panel.summary, &format!("{} entities under {prefix}", entries.len()));

    let live: std::collections::BTreeSet<&str> = panel.wiring.iter().map(|r| r.path.as_str()).collect();
    let mut bundle_lines = String::new();
    let mut bundle_n = 0usize;
    for entry in &entries {
        if live.contains(entry.path.as_str()) {
            continue;
        }
        bundle_n += 1;
        match peers.get_entity(peer_id, &entry.path) {
            Some(entity) => bundle_lines.push_str(&format!(
                "{}  [{}, {}B]\n",
                entry.path,
                entity.entity_type,
                entity.data.len()
            )),
            None => bundle_lines.push_str(&format!("{}  <unreadable>\n", entry.path)),
        }
    }
    util::set_text(&panel.bundle_summary, &format!("static bundle ({bundle_n}, not decoded)"));
    util::set_text(&panel.bundle_pre, &bundle_lines);
}

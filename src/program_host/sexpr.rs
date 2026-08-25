//! Pure compute-IR → s-expression pretty-printer (the debug overlay's renderer,
//! extracted so it is **native-testable**).
//!
//! Lives in `program_host` (native-compilable — the same home as the
//! descriptor/shape decoders it mirrors), NOT in `app_host` (which is
//! `#![cfg(target_arch = "wasm32")]`, so `make test` cannot reach it). The
//! caller (`app_host::debug`) keeps the DOM + `Peers` tree walk and supplies a
//! [`HashResolver`]; everything here is `ciborium::Value` / bytes → `String`
//! with no `web_sys`, so the compute-node → Lisp mapping is covered by the
//! table-driven unit tests at the bottom — the mapping used to be verified by
//! *nothing* (`AUDIT-L5-COMPUTE-HOST-FOUNDATION-2026-08-01` finding #3).
//!
//! Field names/shapes per node type are `EXTENSION-COMPUTE` §2.1–2.2 (mirrored
//! 1:1 in `entity-core-rust/extensions/compute/src/eval/*.rs`, the evaluator
//! that is the actual runtime authority) — this is a *pretty-printer*, a second
//! IR decoder that can drift from that authority, so the tests pin the mapping
//! (D8/D1: no kernel "render IR" service exists to reuse).
//!
//! **Drift ratchet (AUDIT-…-2026-08-01 #11):** because this hand-decodes the
//! node vocabulary the evaluator owns, it is a *parallel decoder*. When the
//! upstream compute IR gains or renames a node type, this `match` and its
//! tests must follow — a missed case does not crash (the `other =>` fallback
//! renders it as `({type} …)`, D13), it just reads less clearly. If this ever
//! grows load-bearing, promote it upstream as a real "render IR" surface rather
//! than letting the two decoders diverge further.

use std::cell::Cell;

/// Resolve a child compute node referenced by content-hash hex. The debug
/// overlay backs this with the peer's `tree_listing` hash index + `get_entity`;
/// a unit test backs it with an in-memory map. Returns the child's
/// `(entity_type, data)` when the hex resolves to a materialized entity.
pub trait HashResolver {
    fn resolve(&self, hex: &str) -> Option<(String, Vec<u8>)>;
}

/// Node-visit budget for one root expression's render — a cheap circuit
/// breaker against a pathological/self-referential graph, independent of
/// [`MAX_EXPR_DEPTH`] (a wide-but-shallow DAG with heavily shared
/// sub-expressions could otherwise blow up the output without ever hitting
/// the depth cap, since a shared node is re-rendered at every place it's
/// referenced — a Lisp-style flattened expansion, same as writing it out by
/// hand). Generous for any real authored program; not reachable in practice.
pub const MAX_EXPR_NODES: usize = 800;
pub const MAX_EXPR_DEPTH: usize = 40;

/// Render `data` (a decoded compute node of `entity_type`) — and everything its
/// hash-refs point at, recursively — as a Lisp-style s-expression. The entry
/// point: seeds a fresh visit budget and calls [`sexpr`].
pub fn render_expr(resolver: &dyn HashResolver, entity_type: &str, data: &[u8]) -> String {
    let budget = Cell::new(MAX_EXPR_NODES);
    sexpr(resolver, &budget, entity_type, data, 0)
}

/// Render one decoded compute-expression node as a Lisp-style s-expression:
/// `(op left right)` for arithmetic/compare/logic (the `op` field IS the
/// natural Lisp head symbol), `(let ((name val) …) body)`, `(if cond then
/// else)`, `(lambda (params…) body)`, `(tree-ref "path")` / a bare `name` for
/// the two lookup forms, `(call "path" :op (arg val) …)` / `(apply fn (arg
/// val) …)` for the two `compute/apply` modes. Anything not in that list (a
/// value type like `compute/result`, or a genuinely unknown type) falls back
/// to `({type} {single-level pretty fields})` rather than vanishing — D13.
fn sexpr(
    resolver: &dyn HashResolver,
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
                match resolver.resolve(&hex) {
                    Some((entity_type, data)) => {
                        sexpr(resolver, budget, &entity_type, &data, depth + 1)
                    }
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

// --- pure CBOR helpers (shared by the renderer and the entity-value preview) ---

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
pub fn decode_preview(data: &[u8]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::Value;
    use std::collections::BTreeMap;

    /// Encode a CBOR value to the byte form `sexpr`/`decode_preview` consume.
    fn enc(v: &Value) -> Vec<u8> {
        let mut buf = Vec::new();
        ciborium::into_writer(v, &mut buf).unwrap();
        buf
    }

    /// A compute node as a CBOR map (`{field: value, …}`), the shape the
    /// materialized store hands the renderer.
    fn node(fields: &[(&str, Value)]) -> Vec<u8> {
        let map = Value::Map(
            fields
                .iter()
                .map(|(k, v)| (Value::Text((*k).into()), v.clone()))
                .collect(),
        );
        enc(&map)
    }

    /// A hash-ref field: a 1-byte tag prefix + the raw hash bytes (the renderer
    /// hexes `bytes[1..]` and looks it up). We use printable ASCII as the hash
    /// so the resolver key is easy to assert against.
    fn href(hash_ascii: &str) -> Value {
        let mut b = vec![0u8]; // tag prefix, skipped by the renderer
        b.extend_from_slice(hash_ascii.as_bytes());
        Value::Bytes(b)
    }

    fn hex_of(hash_ascii: &str) -> String {
        hash_ascii.bytes().map(|x| format!("{x:02x}")).collect()
    }

    /// In-memory resolver: hex → (entity_type, encoded-node-bytes).
    #[derive(Default)]
    struct MapResolver(BTreeMap<String, (String, Vec<u8>)>);
    impl MapResolver {
        fn with(mut self, hash_ascii: &str, ty: &str, data: Vec<u8>) -> Self {
            self.0.insert(hex_of(hash_ascii), (ty.into(), data));
            self
        }
    }
    impl HashResolver for MapResolver {
        fn resolve(&self, hex: &str) -> Option<(String, Vec<u8>)> {
            self.0.get(hex).cloned()
        }
    }

    fn render(resolver: &dyn HashResolver, ty: &str, data: &[u8]) -> String {
        render_expr(resolver, ty, data)
    }

    #[test]
    fn literal_renders_inline_value() {
        let r = MapResolver::default();
        let data = node(&[("value", Value::Integer(42.into()))]);
        assert_eq!(render(&r, "compute/literal", &data), "42");
    }

    #[test]
    fn scope_lookup_is_a_bare_name() {
        let r = MapResolver::default();
        let data = node(&[("name", Value::Text("cell".into()))]);
        assert_eq!(render(&r, "compute/lookup/scope", &data), "cell");
    }

    #[test]
    fn tree_lookup_absolute_and_relative() {
        let r = MapResolver::default();
        let abs = node(&[("path", Value::Text("/world/state".into()))]);
        assert_eq!(render(&r, "compute/lookup/tree", &abs), "(tree-ref \"/world/state\")");
        let rel = node(&[
            ("path", Value::Text("state".into())),
            ("relative", Value::Bool(true)),
        ]);
        assert_eq!(render(&r, "compute/lookup/tree", &rel), "(tree-ref \"state\" :relative)");
    }

    #[test]
    fn arithmetic_binary_uses_op_as_head_and_resolves_children() {
        // (+ 1 2) where both operands are hash-refs to literals.
        let r = MapResolver::default()
            .with("L1", "compute/literal", node(&[("value", Value::Integer(1.into()))]))
            .with("R2", "compute/literal", node(&[("value", Value::Integer(2.into()))]));
        let data = node(&[
            ("op", Value::Text("+".into())),
            ("left", href("L1")),
            ("right", href("R2")),
        ]);
        assert_eq!(render(&r, "compute/arithmetic", &data), "(+ 1 2)");
    }

    #[test]
    fn logic_unary_omits_the_missing_right() {
        let r = MapResolver::default()
            .with("A", "compute/lookup/scope", node(&[("name", Value::Text("alive".into()))]));
        let data = node(&[("op", Value::Text("not".into())), ("left", href("A"))]);
        assert_eq!(render(&r, "compute/logic", &data), "(not alive)");
    }

    #[test]
    fn if_with_and_without_else() {
        let r = MapResolver::default()
            .with("C", "compute/lookup/scope", node(&[("name", Value::Text("c".into()))]))
            .with("T", "compute/lookup/scope", node(&[("name", Value::Text("t".into()))]))
            .with("E", "compute/lookup/scope", node(&[("name", Value::Text("e".into()))]));
        let with_else = node(&[
            ("condition", href("C")),
            ("then", href("T")),
            ("else", href("E")),
        ]);
        assert_eq!(render(&r, "compute/if", &with_else), "(if c\n  t\n  e)");
        let no_else = node(&[("condition", href("C")), ("then", href("T"))]);
        assert_eq!(render(&r, "compute/if", &no_else), "(if c\n  t)");
    }

    #[test]
    fn lambda_lists_params() {
        let r = MapResolver::default()
            .with("B", "compute/lookup/scope", node(&[("name", Value::Text("x".into()))]));
        let data = node(&[
            (
                "params",
                Value::Array(vec![Value::Text("x".into()), Value::Text("y".into())]),
            ),
            ("body", href("B")),
        ]);
        assert_eq!(render(&r, "compute/lambda", &data), "(lambda (x y) x)");
    }

    #[test]
    fn apply_call_mode_uses_path_and_operation() {
        let r = MapResolver::default()
            .with("V", "compute/literal", node(&[("value", Value::Integer(7.into()))]));
        let data = node(&[
            ("path", Value::Text("/lib/add".into())),
            ("operation", Value::Text("invoke".into())),
            (
                "args",
                Value::Map(vec![(Value::Text("n".into()), href("V"))]),
            ),
        ]);
        assert_eq!(
            render(&r, "compute/apply", &data),
            "(call \"/lib/add\" :invoke\n    (n 7)\n  )"
        );
    }

    #[test]
    fn unresolved_hash_ref_is_visible_not_silent() {
        // No resolver entry → the renderer must show a marker, never vanish (D13).
        let r = MapResolver::default();
        let data = node(&[("op", Value::Text("+".into())), ("left", href("MISSING"))]);
        let out = render(&r, "compute/arithmetic", &data);
        assert!(out.starts_with("(+ #unresolved:"), "got: {out}");
    }

    #[test]
    fn unknown_node_type_falls_back_visibly_not_empty() {
        // A type outside the known vocabulary still renders (D13 — never vanish).
        let r = MapResolver::default();
        let data = node(&[("k", Value::Integer(1.into()))]);
        let out = render(&r, "compute/some-future-node", &data);
        assert!(out.starts_with("(compute/some-future-node "), "got: {out}");
    }

    #[test]
    fn depth_cap_emits_a_visible_marker() {
        // A self-referential ref chain longer than MAX_EXPR_DEPTH must stop with
        // the depth marker, not recurse forever.
        let mut r = MapResolver::default();
        // Each node arithmetically refs the next; the last is missing.
        for i in 0..(MAX_EXPR_DEPTH + 5) {
            let next = format!("N{}", i + 1);
            r = r.with(
                &format!("N{i}"),
                "compute/arithmetic",
                node(&[("op", Value::Text("+".into())), ("left", href(&next))]),
            );
        }
        let root = node(&[("op", Value::Text("+".into())), ("left", href("N0"))]);
        let out = render(&r, "compute/arithmetic", &root);
        assert!(out.contains("\u{2026}(depth limit)"), "expected depth marker, got: {out}");
    }

    #[test]
    fn pretty_cbor_inlines_scalar_arrays_and_caps_them() {
        let arr = Value::Array((0..20).map(|n| Value::Integer(n.into())).collect());
        let out = decode_preview(&enc(&arr));
        assert!(out.starts_with("[0, 1, 2,"), "got: {out}");
        assert!(out.contains("\u{2026} (4 more)"), "expected cap marker, got: {out}");
    }

    #[test]
    fn decode_preview_non_cbor_is_labeled_not_errored() {
        // Invalid CBOR head byte → the labeled fallback, never a panic (AP11:
        // a handled fallback, shown as text).
        let out = decode_preview(&[0xff, 0xff, 0xff]);
        assert_eq!(out, "<3 raw bytes, not CBOR>");
    }

    #[test]
    fn decode_preview_truncates_by_char_count() {
        let long: String = "x".repeat(600);
        let out = decode_preview(&enc(&Value::Text(long)));
        // The rendered string is quoted; truncated to 500 chars + ellipsis.
        assert_eq!(out.chars().count(), 501);
        assert!(out.ends_with('\u{2026}'));
    }
}

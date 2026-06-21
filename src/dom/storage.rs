//! Storage window DOM renderer — pure consumer of
//! [`StorageOutput`](crate::views::storage::output::StorageOutput).
//!
//! Read-only dashboard: per hosted peer it shows the content-store blob count,
//! the live-path count, the approximate orphan gap, the save-state churn
//! highlight, and a per-top-level-prefix breakdown; plus the origin disk
//! estimate. The only interactive element is a Refresh button.

use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::storage::output::{OriginEstimate, PeerStorage, StorageOutput};

use web_sys::Element;

const CARD: &str = "background:var(--surface-sunken,#0a0a1a);border:1px solid \
    var(--border,#2a2a4e);border-radius:6px;padding:10px;margin:8px 0";
const STAT_ROW: &str =
    "display:flex;justify-content:space-between;gap:12px;font-size:13px;margin:2px 0";
const BADGE: &str = "font-size:10px;font-weight:bold;padding:1px 6px;border-radius:8px;\
    background:var(--surface,#2a2a4e);color:var(--text-dim,#888);margin-left:8px";

pub fn render(container: &Element, output: &StorageOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "storage");
    wrapper.set_attribute("style", theme::SECTION).ok();

    // Header + Refresh.
    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, "Storage");
    util::append(&header, &h2);
    let refresh = util::create_element("button");
    // Counts update live via subscription; this re-probes the disk estimate.
    util::set_text(&refresh, "Refresh disk usage");
    refresh.set_attribute("style", theme::BTN_SMALL).ok();
    ctx.on_window_event(
        &refresh,
        "click",
        crate::views::storage::REFRESH_EVENT,
        "",
    );
    util::append(&header, &refresh);
    util::append(&wrapper, &header);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Read-only. The content store is append-only — overwriting a path \
         leaves the old value behind; it isn't reclaimed until GC (GUIDE-GC).",
    );
    util::append(&wrapper, &hint);

    // Origin-wide disk estimate.
    if let Some(est) = &output.estimate {
        util::append(&wrapper, &origin_block(est));
    }

    // Per-peer cards.
    if output.peers.is_empty() {
        let empty = util::create_element("p");
        empty.set_attribute("style", theme::HINT).ok();
        util::set_text(&empty, "(no hosted peers)");
        util::append(&wrapper, &empty);
    } else {
        for peer in &output.peers {
            util::append(&wrapper, &peer_card(peer));
        }
    }

    util::append(container, &wrapper);
}

fn origin_block(est: &OriginEstimate) -> Element {
    let block = util::create_element("div");
    block.set_attribute("style", CARD).ok();

    let title = util::create_element("div");
    title.set_attribute("style", "font-weight:bold;font-size:13px;margin-bottom:4px").ok();
    util::set_text(&title, "Origin disk (IndexedDB + caches — whole origin)");
    util::append(&block, &title);

    let pct = if est.quota_bytes > 0.0 {
        format!(" ({:.1}%)", est.usage_bytes / est.quota_bytes * 100.0)
    } else {
        String::new()
    };
    util::append(
        &block,
        &stat_row(
            "Used / quota",
            &format!(
                "{} / {}{pct}",
                format_bytes(est.usage_bytes),
                format_bytes(est.quota_bytes)
            ),
        ),
    );
    let persisted = match est.persisted {
        Some(true) => "yes (eviction-protected)",
        Some(false) => "no (best-effort / evictable)",
        None => "unknown",
    };
    util::append(&block, &stat_row("Persisted", persisted));
    block
}

fn peer_card(peer: &PeerStorage) -> Element {
    let card = util::create_element("div");
    card.set_attribute("style", CARD).ok();

    // Title: short peer id + arm badge.
    let title = util::create_element("div");
    title.set_attribute("style", "margin-bottom:6px;display:flex;align-items:center").ok();
    let id = util::create_element("code");
    id.set_attribute("style", "font-size:12px;color:var(--text-muted,#c0c0c0)").ok();
    util::set_text(&id, &short_id(&peer.peer_id));
    util::append(&title, &id);
    let badge = util::create_element("span");
    badge.set_attribute("style", BADGE).ok();
    util::set_text(&badge, if peer.is_backend { "Worker / OPFS" } else { "Direct / IDB" });
    util::append(&title, &badge);
    util::append(&card, &title);

    // Headline stats.
    util::append(&card, &stat_row("Content-store blobs", &peer.content_blobs.to_string()));
    util::append(&card, &stat_row("Live tree paths", &peer.live_paths.to_string()));
    let orphans = peer.approx_orphans();
    util::append(
        &card,
        &stat_row_colored(
            "Superseded / orphaned blobs (approx.)",
            &orphans.to_string(),
            if orphans > 0 { Some(crate::theme_tokens::STATUS_INFO) } else { None },
        ),
    );
    util::append(
        &card,
        &stat_row_colored(
            "Save-state paths",
            &peer.save_state_paths.to_string(),
            if peer.save_state_paths > 0 { Some(crate::theme_tokens::STATUS_INFO) } else { None },
        ),
    );

    // Per-prefix breakdown (largest first).
    if peer.is_backend && peer.buckets.is_empty() {
        let note = util::create_element("div");
        note.set_attribute("style", theme::HINT).ok();
        util::set_text(&note, "(per-prefix breakdown unavailable on the Worker/OPFS arm)");
        util::append(&card, &note);
    } else if !peer.buckets.is_empty() {
        let sub = util::create_element("div");
        sub.set_attribute("style", "margin-top:6px;font-size:11px;color:var(--text-dim,#888)").ok();
        util::set_text(&sub, "By top-level path:");
        util::append(&card, &sub);

        let mut buckets: Vec<&_> = peer.buckets.iter().collect();
        buckets.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
        for b in buckets {
            util::append(&card, &stat_row(&format!("  {}/", b.label), &b.count.to_string()));
        }
    }

    card
}

fn stat_row(label: &str, value: &str) -> Element {
    stat_row_colored(label, value, None)
}

fn stat_row_colored(label: &str, value: &str, value_color: Option<&str>) -> Element {
    let row = util::create_element("div");
    row.set_attribute("style", STAT_ROW).ok();
    let l = util::create_element("span");
    l.set_attribute("style", "color:var(--text-dim,#888)").ok();
    util::set_text(&l, label);
    util::append(&row, &l);
    let v = util::create_element("span");
    let style = match value_color {
        Some(c) => format!("font-variant-numeric:tabular-nums;color:{c}"),
        None => "font-variant-numeric:tabular-nums".to_string(),
    };
    v.set_attribute("style", &style).ok();
    util::set_text(&v, value);
    util::append(&row, &v);
    row
}

fn short_id(id: &str) -> String {
    if id.len() > 16 {
        format!("{}…{}", &id[..8], &id[id.len() - 6..])
    } else {
        id.to_string()
    }
}

/// Human-readable byte count (binary units).
fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1.0 {
        return "0 B".to_string();
    }
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", value as u64)
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales() {
        assert_eq!(format_bytes(0.0), "0 B");
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1024.0), "1.0 KB");
        assert_eq!(format_bytes(1_572_864.0), "1.5 MB");
    }

    #[test]
    fn short_id_truncates_long_ids() {
        assert_eq!(short_id("abc"), "abc");
        let long = "0123456789abcdef0123456789";
        let s = short_id(long);
        assert!(s.contains('…'));
        assert!(s.starts_with("01234567"));
    }
}

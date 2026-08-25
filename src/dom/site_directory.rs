//! DOM renderer for the site-aware Content Site **window**'s directory rail.
//!
//! A left rail listing every site this peer holds — owned and cached
//! ([`SiteDirectory`]) — alongside the existing single-site browse view. It is
//! a **window-only** surface: the immersive Site Mode overlay deliberately has
//! no directory rail (it shows one site), so this lives here, not in the shared
//! [`crate::dom::content_site`] renderer (which both hosts use untouched).
//!
//! Each row opens its site ([`Action::SiteOpen`]) and carries a bookmark toggle
//! ([`Action::SiteBookmarkToggle`]); cached rows show a provenance subline. The
//! current location is highlighted. Pure construction — no tree/model/peer
//! access; the model assembles the [`SiteDirectory`] the rail reads.

use web_sys::Element;

use crate::action::Action;
use crate::dom::{util, DomCtx};
use crate::views::content_site::output::{RailFilter, SiteDirectory, SiteEntry};
use crate::window::WindowId;

/// Build the directory rail for `window_id` into `rail` from `dir`.
/// `.cs-rail` is a fixed side column on desktop; on mobile it stacks above the
/// content and collapses behind the "Sites" toggle (the responsive CSS is
/// injected — root-scoped — by [`crate::dom::content_site::render`], which the
/// window always calls for the content pane). The toggle is `display:none` on
/// desktop, so the list always shows there.
pub fn render(rail: &Element, dir: &SiteDirectory, ctx: &DomCtx, window_id: WindowId) {
    util::set_attr(rail, "class", "cs-rail");

    let toggle = util::create_element_with_class("button", "cs-rail-toggle");
    util::set_attr(&toggle, "type", "button");
    util::set_text(&toggle, &crate::i18n::t("sitedir.sites_menu", &[]));
    ctx.listen(&toggle, "click", {
        let rail = rail.clone();
        move |evt: web_sys::Event| {
            evt.stop_propagation();
            let _ = rail.class_list().toggle("cs-open");
        }
    });
    util::append(rail, &toggle);

    let list = util::create_element_with_class("div", "cs-rail-list");

    let header = util::create_element("div");
    util::set_text(&header, &crate::i18n::t("sitedir.sites", &[]));
    util::set_attr(
        &header,
        "style",
        "color:var(--site-text-muted-2, #7a8294);font-size:11px;font-weight:600;\
         text-transform:uppercase;letter-spacing:0.06em;padding:0 4px 8px;",
    );
    util::append(&list, &header);

    // My / All / External view filter. `All` is the default (owned + cached
    // together — the historical behaviour); the others narrow the same list.
    util::append(&list, &filter_toggle(dir.filter, ctx, window_id));

    if dir.entries.is_empty() {
        let empty = util::create_element("div");
        // A filtered-but-empty view is different from "no sites at all".
        util::set_text(
            &empty,
            &crate::i18n::t(
                match dir.filter {
                    RailFilter::All => "sitedir.empty_all",
                    RailFilter::Mine => "sitedir.empty_mine",
                    RailFilter::External => "sitedir.empty_external",
                },
                &[],
            ),
        );
        util::set_attr(
            &empty,
            "style",
            "color:var(--site-text-faint-2, #565d6e);font-size:12px;padding:4px;",
        );
        util::append(&list, &empty);
    } else {
        for entry in &dir.entries {
            util::append(&list, &site_row(entry, ctx, window_id));
        }
    }

    util::append(rail, &list);
}

/// The My / All / External segmented control. The active segment is
/// highlighted; clicking a segment dispatches [`Action::SiteRailFilter`].
fn filter_toggle(active: RailFilter, ctx: &DomCtx, window_id: WindowId) -> Element {
    let bar = util::create_element("div");
    util::set_attr(
        &bar,
        "style",
        "display:flex;gap:4px;padding:0 4px 8px;",
    );
    for (label_key, filter) in [
        ("sitedir.filter_mine", RailFilter::Mine),
        ("sitedir.filter_all", RailFilter::All),
        ("sitedir.filter_external", RailFilter::External),
    ] {
        let on = filter == active;
        let btn = util::create_element("button");
        util::set_attr(&btn, "type", "button");
        util::set_text(&btn, &crate::i18n::t(label_key, &[]));
        util::set_attr(
            &btn,
            "style",
            &format!(
                "flex:1;font-size:11px;padding:3px 4px;border-radius:4px;cursor:pointer;\
                 border:1px solid var(--site-border, #20202e);\
                 background:{};color:{};font-weight:{};",
                if on { "var(--site-selected-bg, #1b1b2c)" } else { "transparent" },
                if on {
                    "var(--site-accent, #9fd0ff)"
                } else {
                    "var(--site-text-muted-2, #7a8294)"
                },
                if on { "600" } else { "400" },
            ),
        );
        ctx.on_action(
            &btn,
            "click",
            Action::SiteRailFilter { window_id, filter: filter.as_str().to_string() },
        );
        util::append(&bar, &btn);
    }
    bar
}

/// One directory row: a bookmark toggle + the site name (opens the site) +
/// a provenance/ownership subline.
fn site_row(entry: &SiteEntry, ctx: &DomCtx, window_id: WindowId) -> Element {
    let row = util::create_element("div");
    let bg = if entry.is_current {
        "background:var(--site-selected-bg, #1b1b2c);"
    } else {
        ""
    };
    util::set_attr(
        &row,
        "style",
        &format!(
            "display:flex;align-items:baseline;gap:6px;padding:5px 5px;\
             border-radius:5px;{bg}"
        ),
    );

    // Bookmark toggle (★ filled when bookmarked, ☆ outline otherwise).
    let star = util::create_element("span");
    util::set_text(&star, if entry.bookmarked { "\u{2605}" } else { "\u{2606}" });
    util::set_attr(
        &star,
        "title",
        &crate::i18n::t(
            if entry.bookmarked { "sitedir.bookmark_remove" } else { "sitedir.bookmark_add" },
            &[],
        ),
    );
    util::set_attr(
        &star,
        "style",
        &format!(
            "cursor:pointer;font-size:13px;line-height:1;flex-shrink:0;color:{};",
            if entry.bookmarked { "#e8c34a" } else { "var(--site-text-faint-2, #565d6e)" }
        ),
    );
    ctx.on_action(
        &star,
        "click",
        Action::SiteBookmarkToggle {
            window_id,
            peer: bookkeeping_peer(entry),
            site: entry.site.clone(),
        },
    );
    util::append(&row, &star);

    // Name + subline (the clickable open target).
    let label_col = util::create_element("div");
    util::set_attr(&label_col, "style", "display:flex;flex-direction:column;min-width:0;flex:1;");

    let name = util::create_element("a");
    util::set_text(&name, &entry.site);
    util::set_attr(&name, "href", "#");
    let name_color = if entry.is_current {
        "var(--site-accent, #9fd0ff)"
    } else {
        "var(--site-text-strong, #c3c9d6)"
    };
    let name_weight = if entry.is_current { "font-weight:600;" } else { "" };
    util::set_attr(
        &name,
        "style",
        &format!(
            "color:{name_color};{name_weight}text-decoration:none;font-size:13px;\
             white-space:nowrap;overflow:hidden;text-overflow:ellipsis;"
        ),
    );
    wire_open(ctx, &name, entry, window_id);
    util::append(&label_col, &name);

    let sub = util::create_element("span");
    util::set_text(&sub, &subline(entry));
    util::set_attr(
        &sub,
        "style",
        "color:var(--site-text-faint-2, #565d6e);font-size:10px;white-space:nowrap;\
         overflow:hidden;text-overflow:ellipsis;",
    );
    util::append(&label_col, &sub);
    // Attach the name+subline column to the row. Without this the row renders
    // only the bookmark/keep icons and the site is unidentifiable (the orphaned
    // label_col was built but never appended — the "just shows a bookmark icon"
    // bug).
    util::append(&row, &label_col);

    // Keep-offline toggle — cached sites only (owned sites are always local,
    // so "keep offline" is meaningless for them). ⤓ = full page caching on.
    if !entry.owned {
        let keep = util::create_element("span");
        util::set_text(&keep, "\u{2913}");
        util::set_attr(
            &keep,
            "title",
            &crate::i18n::t(
                if entry.keep_offline { "sitedir.keep_full" } else { "sitedir.keep_pinned" },
                &[],
            ),
        );
        util::set_attr(
            &keep,
            "style",
            &format!(
                "cursor:pointer;font-size:13px;line-height:1;flex-shrink:0;color:{};",
                if entry.keep_offline { "#5fc27e" } else { "var(--site-text-faint-2, #565d6e)" }
            ),
        );
        ctx.on_action(
            &keep,
            "click",
            Action::SiteKeepToggle {
                window_id,
                peer: bookkeeping_peer(entry),
                site: entry.site.clone(),
            },
        );
        util::append(&row, &keep);
    }

    row
}

/// The provenance/ownership subline: `owned` for my sites, `cached · {host}`
/// for fetched foreign sites (the origin host, scheme/path stripped), with a
/// `· N×` visit-count tail once the site has been opened from here.
fn subline(entry: &SiteEntry) -> String {
    let mut s = if entry.owned {
        crate::i18n::t("sitedir.sub_owned", &[])
    } else {
        match host_of(&entry.source_transport) {
            Some(host) => crate::i18n::t("sitedir.sub_cached_from", &[("host", &host)]),
            None => crate::i18n::t("sitedir.sub_cached", &[]),
        }
    };
    // **The verification state rides here, and it is not optional.**
    //
    // `cached · labs.example` on its own is `GUIDE-SERVING-MODE` §8's first
    // forbidden cell: a **transport** fact ("which host served this") rendered as
    // neutral chrome on a row where **nothing has been verified**, which a reader
    // takes as "fine". Host and key are different questions and only the second
    // is what the trust chain makes answerable.
    //
    // A **foreign** site reached over the Site Browser's own fetch path is always
    // the "never checked" state today: the two-hop check proves a body matches the
    // pointer the same origin served — real against corruption, silent about
    // authorship, because the origin supplied the pointer too. So the honest
    // label is *not verified*, and it stays that way until the signed path is
    // wired in (`GUIDE-PUBLISHING-AND-NAMES` §7.3, backlog B-3).
    //
    // An **owned** site is not in this taxonomy at all — it is our own tree, not
    // something an origin served us, and stamping "not verified" on it would be
    // crying wolf on the one case with nothing to verify.
    if !entry.owned {
        s.push_str(" \u{00b7} ");
        s.push_str(&verification_label(entry));
    }
    if entry.visit_count > 0 {
        s.push_str(&format!(" \u{00b7} {}\u{00d7}", entry.visit_count));
    }
    s
}

/// The §8 verification state for a fetched site.
///
/// Three states, **never collapsed into two**. Today the Site Browser can only
/// ever produce the first — it reads no signed root — and this function exists in
/// three-state shape so that wiring the signed path in is a change of *inputs*
/// rather than a new vocabulary invented under deadline.
///
/// The `Verified` arm carries a **date, always**: bare "Verified" is read as
/// "this is current", which is precisely the claim a published root cannot
/// support (a publisher who has not republished and an origin withholding a
/// newer root are byte-identical at our end). The date is what lets a user notice
/// a stale site, and against a withholding origin it is the only detection
/// mechanism there is.
///
/// **Do not add a "last checked N minutes ago" here.** That bounds *our fetch*,
/// not the publisher's republish, and it will be read as the latter — §8 names
/// that specific temptation and refuses it.
fn verification_label(entry: &SiteEntry) -> String {
    match entry.verified_at {
        None => crate::i18n::t("sitedir.sub_not_verified", &[]),
        Some(0) => crate::i18n::t("sitedir.sub_verify_failed", &[]),
        Some(published_at_ms) => crate::i18n::t(
            "sitedir.sub_verified_as_of",
            &[("date", &crate::views::format_day(published_at_ms))],
        ),
    }
}

/// Pull the host out of an origin string (`https://labs.example/x` →
/// `labs.example`) for a compact provenance label. `None` if empty.
fn host_of(origin: &str) -> Option<String> {
    if origin.is_empty() {
        return None;
    }
    let no_scheme = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin);
    let host = no_scheme.split('/').next().unwrap_or(no_scheme);
    (!host.is_empty()).then(|| host.to_string())
}

/// The peer id the prefs/provenance ledgers key by for this entry. Owned sites
/// key by my own id, but the model's `open_site`/`toggle_bookmark` take an
/// **empty** peer to mean "owned/bound" — so an owned row sends `""` and a
/// cached row sends the concrete foreign id.
fn bookkeeping_peer(entry: &SiteEntry) -> String {
    if entry.owned {
        String::new()
    } else {
        entry.peer.clone()
    }
}

/// Wire a click to open the entry's site (root page) in this window.
fn wire_open(ctx: &DomCtx, el: &Element, entry: &SiteEntry, window_id: WindowId) {
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let peer = bookkeeping_peer(entry);
    let site = entry.site.clone();
    ctx.listen(el, "click", move |evt: web_sys::Event| {
        evt.prevent_default();
        actions.borrow_mut().push(Action::SiteOpen {
            window_id,
            peer: peer.clone(),
            site: site.clone(),
        });
        rp();
    });
}

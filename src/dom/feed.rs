//! Feed DOM renderer — pure consumer of
//! [`FeedOutput`](crate::views::feed::output::FeedOutput).
//!
//! **It decides nothing.** Every sentence is a catalog lookup on a key the model
//! chose, including which of the seven attribution verdicts an entry carries —
//! `FEED-R4` is a MUST about what a reader *presents*, so that choice is the
//! rule itself and not presentation polish.

use crate::action::Action;
use crate::dom::components;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::feed::output::{EntryRow, FeedOutput, FeedPanel};

use web_sys::Element;

/// The draft key for the peer-id box. One per window would be better if two
/// Feed windows were ever open at once; today they would share a draft, which
/// is a cosmetic annoyance rather than a correctness problem — the value is read
/// only on the press, and the press carries it.
const PEER_FIELD: &str = "feed_peer";

pub fn render(container: &Element, output: &FeedOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "feed");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, &crate::i18n::t("window.feed", &[]));
    util::append(&wrapper, &h2);

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("feed.hint", &[]));
    util::append(&wrapper, &hint);

    render_follow_form(&wrapper, output, ctx);
    if let Some(notice) = output.notice {
        util::append(&wrapper, &components::notice(&crate::i18n::t(notice.0, &[])));
    }
    render_follow_list(&wrapper, output, ctx);
    render_panel(&wrapper, output, ctx);

    util::append(container, &wrapper);
}

fn render_follow_form(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    let row = util::create_element("div");
    row.set_attribute("style", theme::ROW_INLINE).ok();

    let input = components::text_input(
        ctx,
        PEER_FIELD,
        "",
        &crate::i18n::t("feed.peer_placeholder", &[]),
    );
    let _ = input.set_attribute("data-field", "feed-peer");
    util::append(&row, &input);

    let btn = components::button_el(
        &crate::i18n::t("feed.follow", &[]),
        components::ButtonKind::Primary,
    );
    let _ = btn.set_attribute("data-field", "feed-follow");
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let drafts = ctx.drafts.clone();
        let wid = output.window_id;
        ctx.listen(&btn, "click", move |_| {
            let typed = drafts.borrow().get(PEER_FIELD).cloned().unwrap_or_default();
            // **An empty press still dispatches.** The refusal comes back as a
            // notice, because a button that does nothing at all is the
            // dead-button disease this app has been bitten by before — and
            // `FollowOutcome::NotAPeerId` is a real, distinct sentence.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: "feed_follow".to_string(),
                value: typed,
            });
            rp();
        });
    }
    util::append(&row, &btn);
    util::append(parent, &row);
}

fn render_follow_list(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    let heading = components::subheading(&crate::i18n::t("feed.following", &[]));
    util::append(parent, &heading);

    if output.follows.is_empty() {
        util::append(parent, &components::empty(&crate::i18n::t("feed.no_follows", &[])));
        return;
    }

    let list = util::create_element("div");
    let _ = list.set_attribute("data-field", "feed-follows");
    for row in &output.follows {
        let line = util::create_element("div");
        line.set_attribute("style", theme::ROW_INLINE).ok();

        let open = components::button_el(
            &row.peer_id,
            if row.selected {
                components::ButtonKind::Primary
            } else {
                components::ButtonKind::Secondary
            },
        );
        let _ = open.set_attribute("data-field", "feed-open");
        ctx.on_window_event(&open, "click", "feed_select", &row.peer_id);
        util::append(&line, &open);

        let drop = components::button_el(
            &crate::i18n::t("feed.unfollow", &[]),
            components::ButtonKind::Secondary,
        );
        let _ = drop.set_attribute("data-field", "feed-unfollow");
        ctx.on_window_event(&drop, "click", "feed_unfollow", &row.peer_id);
        util::append(&line, &drop);

        util::append(&list, &line);
    }
    util::append(parent, &list);
}

fn render_panel(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    let Some(selected) = &output.selected else {
        util::append(
            parent,
            &components::empty(&crate::i18n::t("feed.nobody_selected", &[])),
        );
        return;
    };

    let head = util::create_element("div");
    head.set_attribute("style", theme::ROW_INLINE).ok();
    util::append(&head, &components::subheading(selected));
    // **`btn.refresh`, not a `feed.refresh` of our own.** Minting a second key
    // for a word the catalog already has is C15's drift inside the catalog, and
    // `i18n-locale-check` caught it: four locales had already rendered
    // "Refresh" one way, and a fresh key let this surface render it another. One
    // English word, one key.
    let refresh = components::button_el(
        &crate::i18n::t("btn.refresh", &[]),
        components::ButtonKind::Secondary,
    );
    let _ = refresh.set_attribute("data-field", "feed-refresh");
    ctx.on_window_event(&refresh, "click", "feed_refresh", selected);
    util::append(&head, &refresh);
    util::append(parent, &head);

    match &output.panel {
        // Unreachable here (the `else` above covers it), and matched rather
        // than `_`-ed so a new panel variant is a compile error.
        FeedPanel::NobodySelected => {}
        FeedPanel::NoRoute => {
            util::append(parent, &components::notice(&crate::i18n::t("feed.no_route", &[])));
        }
        FeedPanel::Loading => {
            util::append(parent, &components::loading(&crate::i18n::t("feed.loading", &[])));
        }
        FeedPanel::NoPosts => {
            util::append(parent, &components::empty(&crate::i18n::t("feed.no_posts", &[])));
        }
        FeedPanel::Failed { detail } => {
            util::append(parent, &components::error(&crate::i18n::t("feed.failed", &[])));
            let d = util::create_element("div");
            d.set_attribute("style", theme::HINT).ok();
            let _ = d.set_attribute("data-field", "feed-error-detail");
            // The transport's own words, beside a localized heading rather than
            // in place of one — it is a diagnostic, not a sentence we authored.
            util::set_text(&d, detail); // i18n-ignore — verbatim transport detail
            util::append(parent, &d);
        }
        FeedPanel::Entries(rows) => render_entries(parent, rows),
    }
}

fn render_entries(parent: &Element, rows: &[EntryRow]) {
    let list = util::create_element("div");
    let _ = list.set_attribute("data-field", "feed-entries");
    let _ = list.set_attribute("data-count", &rows.len().to_string());

    for row in rows {
        let card = components::card("");
        let _ = card.set_attribute("data-field", "feed-entry");

        let body = util::create_element("div");
        // The authored `fallback` — EMBED §3 makes it mandatory and non-empty
        // exactly so a reader with no renderer for the payload has a sentence.
        util::set_text(&body, &row.text); // i18n-ignore — the author's own words
        util::append(&card, &body);

        let meta = util::create_element("div");
        meta.set_attribute("style", theme::HINT).ok();
        let _ = meta.set_attribute("data-field", "feed-entry-meta");
        let _ = meta.set_attribute(
            "data-attributed",
            if row.attributed { "true" } else { "false" },
        );
        // **The verdict is rendered for every entry, including the good one.**
        // A surface that showed a chip only when something was wrong would make
        // *"this is verified"* indistinguishable from *"nobody looked"*.
        util::set_text(
            &meta,
            &format!(
                "{} · {}",
                row.id_short, // i18n-ignore — a content hash
                crate::i18n::t(row.attribution_key, &[])
            ),
        );
        util::append(&card, &meta);

        util::append(&list, &card);
    }
    util::append(parent, &list);
}

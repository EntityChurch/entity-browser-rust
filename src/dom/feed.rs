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
use crate::views::feed::output::{EntryRow, FeedOutput, FeedPanel, Via};

use web_sys::Element;

/// The draft key for the peer-id box. One per window would be better if two
/// Feed windows were ever open at once; today they would share a draft, which
/// is a cosmetic annoyance rather than a correctness problem — the value is read
/// only on the press, and the press carries it.
const PEER_FIELD: &str = "feed_peer";

/// The draft key for the gatherer box. **Its own field, not `PEER_FIELD`
/// reused** — one box for two lists would let somebody type a peer id, press
/// *Read through*, and have the Follow box they were looking at appear to
/// change meaning under them.
const GATHERER_FIELD: &str = "feed_gatherer";

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

    render_composer(&wrapper, output, ctx);
    render_follow_form(&wrapper, output, ctx);
    if let Some(notice) = output.notice {
        util::append(&wrapper, &components::notice(&crate::i18n::t(notice.0, &[])));
    }
    render_follow_list(&wrapper, output, ctx);
    render_gatherers(&wrapper, output, ctx);
    render_panel(&wrapper, output, ctx);

    util::append(container, &wrapper);
}

/// The draft key for the composer box. Its own field for `GATHERER_FIELD`'s
/// reason — three boxes, three drafts, so no press can read what was typed into
/// a different one.
const POST_FIELD: &str = "feed_post";

/// **The composer** — post into your own tree, and unpublish what is there.
///
/// ⭐ Rendered **above** the reading surface, which is the one layout decision
/// here that is not arbitrary: this is the only part of the window that acts on
/// *your* tree, and burying it under two lists of other people's peer ids reads
/// as an afterthought on a surface whose whole other half is reading.
///
/// ⛔ **`FEED-R21` lives on the Remove button's own notice, at the moment of the
/// action.** §7.5 is explicit that the honest sentence belongs there and not in
/// a help page, and the model hands it back from the verb
/// ([`crate::feed_compose::RemovalMeaning`]) so this renderer cannot forget to
/// ask for it — it renders whatever key the model chose, and for a removal that
/// key is the unpublication sentence.
fn render_composer(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    let heading = components::subheading(&crate::i18n::t("feed.compose.heading", &[]));
    util::append(parent, &heading);

    if !output.can_author {
        // **Says so rather than rendering a dead box.** This profile holds no
        // authoring key for the bound peer, which is not something retrying or
        // typing differently fixes, so offering the control would be offering
        // an act that cannot succeed.
        let note = components::empty(&crate::i18n::t("feed.compose.not_our_peer", &[]));
        let _ = note.set_attribute("data-field", "feed-compose-unavailable");
        util::append(parent, &note);
        return;
    }

    let row = util::create_element("div");
    row.set_attribute("style", theme::ROW_INLINE).ok();

    let input = components::text_input(
        ctx,
        POST_FIELD,
        "",
        &crate::i18n::t("feed.compose.placeholder", &[]),
    );
    let _ = input.set_attribute("data-field", "feed-post-text");
    util::append(&row, &input);

    let btn = components::button_el(
        &crate::i18n::t("feed.compose.post", &[]),
        components::ButtonKind::Primary,
    );
    let _ = btn.set_attribute("data-field", "feed-post");
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let drafts = ctx.drafts.clone();
        let wid = output.window_id;
        ctx.listen(&btn, "click", move |_| {
            let typed = drafts.borrow().get(POST_FIELD).cloned().unwrap_or_default();
            // An empty press still dispatches, for the Follow button's reason:
            // the refusal is a real, distinct sentence and a button that does
            // nothing at all is worse than one that says why.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: "feed_post".to_string(),
                value: typed,
            });
            rp();
        });
    }
    util::append(&row, &btn);
    util::append(parent, &row);

    if let Some(notice) = output.compose_notice {
        let n = components::notice(&crate::i18n::t(notice.key(), &[]));
        // Read as an attribute, never by matching the sentence — a gate that
        // matched copy would be red the day the wording is translated or
        // improved, which is the needle defect one subsystem over.
        let _ = n.set_attribute("data-field", "feed-compose-notice");
        util::append(parent, &n);
    }

    if output.own_posts.is_empty() {
        util::append(parent, &components::empty(&crate::i18n::t("feed.compose.no_posts", &[])));
        return;
    }

    let list = util::create_element("div");
    let _ = list.set_attribute("data-field", "feed-own-posts");
    for post in &output.own_posts {
        let line = util::create_element("div");
        line.set_attribute("style", theme::ROW_INLINE).ok();

        let text = util::create_element("div");
        let _ = text.set_attribute("data-field", "feed-own-post-text");
        util::set_text(&text, &post.text);
        util::append(&line, &text);

        let id = util::create_element("span");
        id.set_attribute("style", theme::HINT).ok();
        let _ = id.set_attribute("data-field", "feed-own-post-id");
        util::set_text(&id, &post.id_short);
        util::append(&line, &id);

        let drop = components::button_el(
            &crate::i18n::t("feed.compose.remove", &[]),
            components::ButtonKind::Secondary,
        );
        let _ = drop.set_attribute("data-field", "feed-remove-post");
        // ⚠ **The FULL hash, not `id_short`.** §7.3's unbinding is by address
        // and a shortened hash names no binding — a remove built from one would
        // unbind nothing and report the unpublication sentence anyway.
        ctx.on_window_event(&drop, "click", "feed_remove_post", &post.hash_hex);
        util::append(&line, &drop);

        util::append(&list, &line);
    }
    util::append(parent, &list);
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

/// **Who this profile reads other authors through** — §6's third source leg.
///
/// A second list rather than a flag on the first, because it answers a different
/// question: you *follow* Alice to read Alice, and you name Greg to be able to
/// read **anybody** through Greg. The section says what it is for, because a
/// list of peer ids with no explanation is a list nobody can use correctly.
fn render_gatherers(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    util::append(parent, &components::subheading(&crate::i18n::t("feed.gatherers", &[])));

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("feed.gatherers_hint", &[]));
    util::append(parent, &hint);

    let row = util::create_element("div");
    row.set_attribute("style", theme::ROW_INLINE).ok();
    let input = components::text_input(
        ctx,
        GATHERER_FIELD,
        "",
        &crate::i18n::t("feed.gatherer_placeholder", &[]),
    );
    let _ = input.set_attribute("data-field", "feed-gatherer-peer");
    util::append(&row, &input);

    let btn = components::button_el(
        &crate::i18n::t("feed.add_gatherer", &[]),
        components::ButtonKind::Secondary,
    );
    let _ = btn.set_attribute("data-field", "feed-add-gatherer");
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let drafts = ctx.drafts.clone();
        let wid = output.window_id;
        ctx.listen(&btn, "click", move |_| {
            let typed = drafts.borrow().get(GATHERER_FIELD).cloned().unwrap_or_default();
            // An empty press still dispatches — the refusal is a real sentence,
            // and a button that does nothing is the dead-button disease.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: "feed_add_gatherer".to_string(),
                value: typed,
            });
            rp();
        });
    }
    util::append(&row, &btn);
    util::append(parent, &row);

    if output.gatherers.is_empty() {
        util::append(parent, &components::empty(&crate::i18n::t("feed.no_gatherers", &[])));
        return;
    }

    let list = util::create_element("div");
    let _ = list.set_attribute("data-field", "feed-gatherers");
    for g in &output.gatherers {
        let line = util::create_element("div");
        line.set_attribute("style", theme::ROW_INLINE).ok();

        let label = util::create_element("span");
        let _ = label.set_attribute("data-field", "feed-gatherer");
        let _ = label.set_attribute("data-routed", if g.routed { "true" } else { "false" });
        util::set_text(&label, &g.peer_id); // i18n-ignore — a peer id
        util::append(&line, &label);

        // **An unrouted gatherer says so rather than disappearing.** It
        // contributes no leg (there is no URL to build, and inventing one
        // relative to the page is `OriginFeedSource`'s empty-origin defect), and
        // a row that silently did nothing would leave somebody wondering why
        // adding it changed nothing.
        if !g.routed {
            let warn = util::create_element("span");
            warn.set_attribute("style", theme::HINT).ok();
            let _ = warn.set_attribute("data-field", "feed-gatherer-unrouted");
            util::set_text(&warn, &crate::i18n::t("feed.gatherer_no_route", &[]));
            util::append(&line, &warn);
        }

        let drop = components::button_el(
            &crate::i18n::t("feed.remove_gatherer", &[]),
            components::ButtonKind::Secondary,
        );
        let _ = drop.set_attribute("data-field", "feed-remove-gatherer");
        ctx.on_window_event(&drop, "click", "feed_remove_gatherer", &g.peer_id);
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
        FeedPanel::Entries { via, rows } => render_entries(parent, via, rows),
    }
}

fn render_entries(parent: &Element, via: &Via, rows: &[EntryRow]) {
    // **Which leg served this, on screen rather than only in the log.** A live
    // read is as fresh as the author is; a published one is as fresh as their
    // last publish; a mirror is somebody else's reading, which §6.1 rule 2
    // allows to be short. Somebody asking *"am I seeing everything?"* cannot
    // answer it without knowing which they got.
    //
    // ⛔ **This line is the ONE place a gatherer's peer id may appear** — §6.1
    // rule 3 / `FEED-R13`: attribution follows each entry's own detached
    // signature, and a surface naming the gatherer as the author is
    // non-conformant. `EntryRow` carries no field it could travel in, so the
    // rule is a property of the types rather than one this renderer remembers.
    let src = util::create_element("div");
    src.set_attribute("style", theme::HINT).ok();
    let _ = src.set_attribute("data-field", "feed-via");
    let _ = src.set_attribute("data-via", via.key());
    let text = match via.source_peer() {
        Some(gatherer) => crate::i18n::t(via.key(), &[("peer", gatherer)]),
        None => crate::i18n::t(via.key(), &[]),
    };
    util::set_text(&src, &text);
    util::append(parent, &src);

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

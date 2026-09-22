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
use crate::feed_body::BodyRender;
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
    // ⭐⭐ **THE ANTI-VACUITY ANCHOR, AND IT IS THE THIRD SPELLING.** A gate
    // reading this window needs one marker meaning *the body rendered at all*,
    // because the window CHROME draws the title and `(no body)` would otherwise
    // be indistinguishable from a state that merely lacks the words being
    // looked for.
    //
    // It was a copy string (`"Follow a publisher by peer id"`) and went red the
    // day that hint was reworded — so it became `[data-field="feed-peer"]`, the
    // Follow input, on the reasoning that only this module draws it. **True and
    // still wrong**, because it is drawn *conditionally*: the 2026-09-16 reorder
    // put that input behind the collapsed *Manage sources* header and three
    // gates reported `RED (VACUOUS)` against a window that had rendered
    // perfectly.
    //
    // ⇒ **an anchor must be drawn UNCONDITIONALLY by the function under test,**
    // not merely drawn by it — "only we draw it" is a uniqueness claim and the
    // property a gate needs is *always*. This element exists for no other
    // purpose, which is what makes it the last spelling.
    let _ = wrapper.set_attribute("data-field", "feed-body-root");

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, &crate::i18n::t("window.feed", &[]));
    util::append(&wrapper, &h2);

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("feed.hint", &[]));
    util::append(&wrapper, &hint);

    // ⭐ **Reading first, administering last — the order IS the complaint.**
    // Until 2026-09-16 this window rendered composer · browse · follow-form ·
    // notice · follow-list · gatherers · **panel**, so the feed a person came to
    // read was the seventh thing on screen, under three text inputs and two
    // lists. *"One little single line is the feed."* The window's subject is
    // what you are reading; everything that configures it is secondary and now
    // sits behind one header.
    //
    // The notice stays at the top because it is the answer to the last thing
    // pressed — inside a section that can be collapsed, a refusal would be
    // reported into a box nobody has open.
    if let Some(notice) = output.notice {
        util::append(&wrapper, &components::notice(&crate::i18n::t(notice.0, &[])));
    }
    render_known(&wrapper, output, ctx);
    render_panel(&wrapper, output, ctx);
    render_composer(&wrapper, output, ctx);

    // The admin, behind one header. `collapsible_header` and NOT
    // `components::disclosure`: a `<details>` re-renders closed on every
    // repaint, and this section holds two inputs somebody types into — which
    // that atom's own doc names as the case it is wrong for.
    util::append(
        &wrapper,
        &components::collapsible_header(
            ctx,
            &crate::i18n::t("feed.manage", &[]),
            output.manage_open,
            "feed_toggle_manage",
        ),
    );
    if output.manage_open {
        let manage = util::create_element("div");
        let _ = manage.set_attribute("data-field", "feed-manage");
        render_follow_form(&manage, output, ctx);
        render_follow_list(&manage, output, ctx);
        render_gatherers(&manage, output, ctx);
        util::append(&wrapper, &manage);
    }

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
/// Place one entry body — EMBED §6's ladder, decided by
/// [`crate::feed_body::decide`] and only *placed* here.
///
/// ⛔ **`set_inner_html` is reachable from exactly ONE arm, and the variant is
/// the safety claim.** `BodyRender::Markup` is constructible only by
/// `feed_body`, which produces it only from
/// [`crate::content_site::markdown_to_html`] — the sanitizer the Site Browser
/// uses, with the XSS tests on it. A renderer that matched on the media type
/// here instead would be a second place that decides what is safe to inject,
/// which is C15 with a sanitizer on the end of it.
fn place_body(card: &Element, body: &BodyRender) {
    let el = util::create_element("div");
    let _ = el.set_attribute("data-field", "feed-body");
    match body {
        BodyRender::Markup(html) => {
            let _ = el.set_attribute("data-render", "markup");
            el.set_inner_html(html);
        }
        BodyRender::Text(text) => {
            let _ = el.set_attribute("data-render", "text");
            util::set_text(&el, text); // i18n-ignore — the author's own words
        }
        BodyRender::Fallback { text, reason } => {
            // **The rung is named in an attribute.** A person sees the authored
            // sentence either way; a gate — and an operator asking *"why am I
            // seeing alt text?"* — needs to tell *we have no renderer for this*
            // from *the author declared something we refuse to run* (AP40).
            let _ = el.set_attribute("data-render", "fallback");
            let _ = el.set_attribute("data-fallback-reason", reason.key());
            // **The miss is a SECOND attribute, not a second reason key.** All
            // three land on `not-in-hand` because that is what the reader is
            // doing; *who could fix it* is the orthogonal fact, and merging the
            // two axes into one key would make `nobody looked` and `the
            // publisher's closure is broken` the same string.
            if let crate::feed_body::FallbackReason::PayloadNotInHand { miss } = reason {
                let _ = el.set_attribute("data-fallback-miss", miss.key());
            }
            util::set_text(&el, text); // i18n-ignore — the author's own words
        }
    }
    util::append(card, &el);
}

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

    // ⭐ **A textarea, not a one-line input.** The body is markdown — it has
    // line breaks by definition — and it was a `text_input` stretched to the
    // full width of the window, which is the widest possible way to show the
    // least possible text. `tracked_textarea` writes into the same
    // `ctx.drafts` map under the same key, so the press below is unchanged and
    // typing still survives a rebuild.
    //
    // `COMPOSE_BOX` rather than `theme::TEXTAREA`: that one is `min-height:300px`
    // because its callers are document editors, and a post box that opens at
    // the height of an article is a surface asking for an essay.
    let input = util::tracked_textarea(&row, ctx, POST_FIELD, "", theme::COMPOSE_BOX);
    util::set_attr(&input, "placeholder", &crate::i18n::t("feed.compose.placeholder", &[]));
    // Set AFTER `tracked_textarea`, which writes the field id here first; the
    // draft listener is keyed on the id it captured, not on this attribute.
    let _ = input.set_attribute("data-field", "feed-post-text");

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

        // ⭐ **Your own post goes through the SAME ladder a stranger's does.**
        // One code path for republished and authored content is
        // `SYSTEM-DATA-EXCHANGE` §1.1's rule, and this is the cheapest place to
        // break it — a composer that previewed its own markdown by a second
        // route would drift from what every reader of your feed sees.
        let text = util::create_element("div");
        let _ = text.set_attribute("data-field", "feed-own-post-text");
        place_body(&text, &post.body);
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

/// ⭐ **The browse list — every publisher this profile can already reach.**
///
/// This is the section that makes the window a reader instead of a text box. It
/// is rendered **above** the Follow form deliberately: the form is the escape
/// hatch for a publisher we have no route to, and a surface that leads with it
/// tells an arriving visitor that the normal way in is to paste 45 characters
/// they do not have.
///
/// Each row is two acts, kept apart for the reason `aim` already draws one
/// button over: **Read** shows you somebody, **Follow** keeps them. *Naming a
/// source is not asking to keep reading them*, so nothing here writes to the
/// durable registry without a press that says so.
fn render_known(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    util::append(parent, &components::subheading(&crate::i18n::t("feed.known", &[])));

    if output.known.is_empty() {
        // ⛔ **Not "this deployment knows nobody" and not silence.** An empty
        // browse list on a profile that has not adopted any origin yet is a
        // routing fact about the reader, not a claim about the world — and the
        // Follow box below is the answer, so the empty state points at it.
        util::append(parent, &components::empty(&crate::i18n::t("feed.no_known", &[])));
        return;
    }

    // ⭐ **S7: a repeated record is a TABLE.** Nine surfaces here already call
    // `components::table`, and the Feed window's three older lists are
    // free-form `flex` rows — S8's own complaint (*"stop making up every new
    // window like we don't have other windows to align with"*). A fourth
    // hand-rolled list would have been the easy consistency and the wrong one:
    // this list has real columns (who, what they are to you, what you can do),
    // which is exactly the case S7 exists for. The three older ones are
    // pre-existing debt, named in `AGENTS.md`, and not silently inherited here.
    let (table, body) = components::table(&[
        &crate::i18n::t("feed.known.col.publisher", &[]),
        &crate::i18n::t("feed.known.col.relation", &[]),
        &crate::i18n::t("feed.known.col.actions", &[]),
    ]);
    let _ = table.set_attribute("data-field", "feed-known");
    for row in &output.known {
        // ⭐⭐ **THE PEER ID IS NOT A BUTTON LABEL.** It was: a 45-character
        // identifier as the caption of the Open control, which is the worst of
        // both — a person cannot read it, and cannot select it to copy either,
        // because clicking selects the publisher instead.
        //
        // ⛔ **And the fix is NOT to shorten it.** `views::short_pid` exists and
        // is wrong here: half an id still cannot be read *and* can no longer be
        // copied, so an abbreviation costs the only thing the string was still
        // good for. There is no name to show in its place — see
        // [`crate::views::feed::output::Selection::heading_key`] for why that is
        // a held decision at arch (`F-6`) and not an omission here.
        //
        // So it goes in `components::copy_code`, the atom whose own doc names
        // this exact failure one surface over: *the table that lists the peers
        // you can transfer files to rendered only a friendly name, so "which
        // peer is this" and "let me send it to the other machine" had no answer
        // on the screen that raised the question.* Same defect, inverted.
        let open = components::button_el(
            &crate::i18n::t("feed.read", &[]),
            if row.selected {
                components::ButtonKind::Primary
            } else {
                components::ButtonKind::Secondary
            },
        );
        let _ = open.set_attribute("data-field", "feed-known-open");
        // ⭐ **The id moved to an attribute because a gate was reading the
        // label.** `known_ids` in the browser gate mapped this button's
        // `textContent`, which was the peer id — so the routed-set rule, whose
        // whole claim is *which* peers appear, rested on a caption. The probe
        // reads `data-peer` now. *Attributes, never sentences* — the line two
        // comments down has said so about `data-home` since the row shipped,
        // and this was the one field still doing it the other way.
        let _ = open.set_attribute("data-peer", &row.peer_id);
        // Attributes, never sentences — a gate reading a copy string is a gate
        // that reds the day somebody rewords it. `home`/`own`/`followed` are the
        // three facts a test needs and a person reads off the labels.
        let _ = open.set_attribute("data-home", if row.home { "1" } else { "0" });
        let _ = open.set_attribute("data-own", if row.own { "1" } else { "0" });
        let _ = open.set_attribute("data-followed", if row.followed { "1" } else { "0" });
        ctx.on_window_event(&open, "click", "feed_select", &row.peer_id);

        // What this peer IS to this profile, in its own column. Both can be
        // true (a deployment publishing under the profile's own peer), so they
        // are joined rather than treated as one enum — and an empty cell is the
        // ordinary case, which a column makes readable and a trailing chip did
        // not.
        let relation: Vec<String> = [(row.home, "feed.known.home"), (row.own, "feed.known.own")]
            .into_iter()
            .filter(|(flag, _)| *flag)
            .map(|(_, key)| crate::i18n::t(key, &[]))
            .collect();

        // **Follow is offered only where it would do something**, and *"where"*
        // is three-valued rather than two — see
        // [`crate::views::feed::output::relation`]. A followed row gets
        // Unfollow; **our own row gets nothing**, because the registry verb
        // refuses our own peer (`FollowOutcome::ThatIsYou`) and this table
        // offered that button for its whole life: a control whose only outcome
        // is a refusal notice.
        //
        // One expression, shared with the panel head, so the two places a
        // person can follow the same peer cannot disagree about them.
        // The follow control itself, or an empty span where there is none —
        // **an element, not a `<td>`**: it shares the Actions cell with Open.
        let follow_control = match crate::views::feed::output::relation(row.own, row.followed) {
            crate::views::feed::output::Relation::Own => util::create_element("span"),
            rel => {
                let (label, event) = match rel {
                    crate::views::feed::output::Relation::Followed => {
                        ("feed.unfollow", "feed_unfollow")
                    }
                    _ => ("feed.follow", "feed_follow"),
                };
                let act = components::button_el(
                    &crate::i18n::t(label, &[]),
                    components::ButtonKind::Secondary,
                );
                let _ = act.set_attribute("data-field", "feed-known-follow");
                // **The row's peer id, not the draft box.** `feed_follow`
                // normally carries what was typed; here the value is the row, so
                // a press cannot follow whatever happens to be left in the
                // Follow input.
                ctx.on_window_event(&act, "click", event, &row.peer_id);
                act
            }
        };

        util::append(
            &body,
            &components::tr(vec![
                components::td(&components::copy_code(ctx, &row.peer_id, None)),
                components::td_text(&relation.join(" · ")),
                components::td(&{
                    // Open and Follow in one cell: the column header says
                    // *Actions*, and two of them is what this row can do.
                    let acts = util::create_element("span");
                    acts.set_attribute("style", theme::ROW_INLINE).ok();
                    util::append(&acts, &open);
                    util::append(&acts, &follow_control);
                    acts
                }),
            ]),
        );
    }
    util::append(parent, &table);
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
        // ⛔ **Two empty states that pointed in opposite directions — TWO
        // FACTS, not one.** With no routed publishers the browse list says
        // *"Follow one by peer id below"* and this said *"Choose a publisher
        // above"*, stacked one line apart, on a screen where there is nothing
        // above. Observed on the running build, which is the only place it is
        // visible: each sentence is correct alone and no test reads two of them
        // together.
        //
        // ⚠ **The first fix was to render nothing when the list is empty, and
        // that was worse** — a silent panel is indistinguishable from one that
        // is still loading or broken, which is the collapse this whole surface
        // is written against. *Nobody has chosen* and *there is nobody to
        // choose* are two states and they get two sentences.
        //
        // The marker is an attribute, not the wording: the control that reads
        // this screen asserted `contains("Choose a publisher")` — an English
        // copy string in a thirty-locale app — and went red on the first change
        // to a sentence, which is the anchor mistake `feed-body-root` above
        // already records twice.
        let none_to_choose = output.known.is_empty();
        let el = components::empty(&crate::i18n::t(
            if none_to_choose { "feed.nothing_to_read" } else { "feed.nobody_selected" },
            &[],
        ));
        let _ = el.set_attribute("data-field", "feed-nobody");
        let _ = el.set_attribute(
            "data-reason",
            if none_to_choose { "no-publishers" } else { "none-chosen" },
        );
        util::append(parent, &el);
        return;
    };

    let head = util::create_element("div");
    head.set_attribute("style", theme::ROW_INLINE).ok();
    // ⭐ **A heading that says something, and the id in full underneath.** This
    // was `subheading(&peer_id)` — a 45-character identifier as the section
    // title, which tells a person nothing and sets the width of the window.
    // The heading now carries the one true thing we know about this publisher
    // (*you* / *this deployment's publisher* / *publisher*); the id follows in
    // `copy_code`, complete and copyable. See
    // [`crate::views::feed::output::Selection::heading_key`] for why there is no
    // name to put there and why that is a held decision rather than a gap here.
    util::append(
        &head,
        &components::subheading(&crate::i18n::t(selected.heading_key(), &[])),
    );
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
    ctx.on_window_event(&refresh, "click", "feed_refresh", &selected.peer_id);
    util::append(&head, &refresh);

    // ⭐⭐ **Follow, on the publisher you are reading.** Until 2026-09-16 the
    // only follow control was in the browse table (peers this deployment
    // already routes to) or in the paste-a-peer-id box behind *Manage
    // sources* — so the journey the product is built for (a registry resolves
    // a name → `publication_probe` says they publish a feed → *Open in Feed*
    // spawns this window aimed at them) **ended one click short of the thing it
    // exists for**, and the missing click was the one that costs a person
    // anything to reproduce by hand: they would have had to find that peer id
    // again, somewhere else.
    //
    // `FeedWindow::aim` selects and deliberately does **not** follow
    // (`aiming_at_a_publisher_does_not_follow_them`) — reading somebody is not
    // subscribing to them, and that is right. This is the control that makes
    // the second act available where the first one landed.
    match crate::views::feed::output::relation(selected.own, selected.followed) {
        // Us. `feed_follows::follow` refuses our own peer, so a button here
        // could only ever produce a refusal notice.
        crate::views::feed::output::Relation::Own => {}
        rel => {
            let (label, event) = match rel {
                crate::views::feed::output::Relation::Followed => {
                    ("feed.unfollow", "feed_unfollow")
                }
                _ => ("feed.follow", "feed_follow"),
            };
            let act = components::button_el(
                &crate::i18n::t(label, &[]),
                components::ButtonKind::Primary,
            );
            let _ = act.set_attribute("data-field", "feed-panel-follow");
            // **Attributes, never the label** — a gate reading the word
            // "Follow" reds the day it is reworded, and the same button in two
            // states is exactly where that bites.
            let _ = act.set_attribute(
                "data-followed",
                if selected.followed { "1" } else { "0" },
            );
            // The peer on screen, never the draft box: `feed_follow` normally
            // carries what was typed into *Manage sources*, and a press here
            // must act on who you are reading.
            ctx.on_window_event(&act, "click", event, &selected.peer_id);
            util::append(&head, &act);
        }
    }
    util::append(parent, &head);

    // The publisher's id, in full and copyable — the only handle that exists
    // for them today, so it is presented as a value rather than hidden in a
    // caption. `data-field` so a gate can read it without reading a label.
    let id_row = components::copy_code(ctx, &selected.peer_id, None);
    let _ = id_row.set_attribute("data-field", "feed-panel-peer");
    util::append(parent, &id_row);

    // ⭐ **What following does, at the control rather than in a help page.** The
    // question *"I clicked follow — what does that mean?"* had no answer
    // anywhere on this surface. A follow writes one row into **your own** tree;
    // its whole effect is that this publisher joins the set the panel reads on
    // each refresh, over whichever leg is live for them. It notifies nobody,
    // subscribes to nothing, and pulls nothing in the background — and a person
    // who assumes otherwise is owed the correction here, where they are about
    // to press it.
    if !selected.own {
        let what = util::create_element("div");
        what.set_attribute("style", theme::HINT).ok();
        let _ = what.set_attribute("data-field", "feed-follow-meaning");
        util::set_text(&what, &crate::i18n::t("feed.follow.meaning", &[]));
        util::append(parent, &what);
    }

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
    // ⭐ **A reading column.** These cards carry prose, and at the width of a
    // maximized window a paragraph is one line the eye cannot return from.
    // `READING_COLUMN` is the Site Browser's own document measure, which is the
    // point: a post and a page should read the same. **The tables above are
    // deliberately NOT constrained** — a browse row has columns and wants the
    // width it is given.
    list.set_attribute("style", theme::READING_COLUMN).ok();
    let _ = list.set_attribute("data-field", "feed-entries");
    let _ = list.set_attribute("data-count", &rows.len().to_string());

    for row in rows {
        let card = components::card("");
        let _ = card.set_attribute("data-field", "feed-entry");

        place_body(&card, &row.body);

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

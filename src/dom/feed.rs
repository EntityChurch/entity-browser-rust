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
use crate::views::feed::output::{EntryRow, FeedOutput, FeedPanel, FeedTab, Selection, Via};

use wasm_bindgen::JsCast;
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

    // ⭐⭐ **THREE PANES, NOT THREE STACKED SECTIONS — and the difference is
    // whether a publisher's archive is between you and the rest of the window.**
    //
    // The 2026-09-16/17 reorder put *Your feed* and *Manage sources* behind
    // collapsible headers, in that order, **below the reading surface**. That
    // was the right diagnosis (reading first, administering last) and the wrong
    // instrument: a disclosure stacks, so with a real feed on screen — 34 posts
    // in the published rig, and that is a small one — both headers sat a screen
    // and a half down, and getting to your own posts meant scrolling past
    // everything somebody else had written. **No ordering of a stack fixes
    // that**, because whichever pane is second is under the first one's content.
    //
    // A tab is *instead*; a disclosure is *extra*. These three are alternatives,
    // so they are tabs, and every one of them is one click from every other one
    // whatever is on screen.
    // The captions are looked up first and held: `i18n::t` returns an owned
    // `String` and a `Tab` borrows its label, so building both in one expression
    // would borrow a temporary.
    let labels: Vec<String> =
        FeedTab::ALL.iter().map(|t| crate::i18n::t(t.label_key(), &[])).collect();
    let tab_items: Vec<components::Tab<'_>> = FeedTab::ALL
        .iter()
        .zip(labels.iter())
        .map(|(t, label)| components::Tab {
            label,
            value: t.value(),
            selected: *t == output.tab,
        })
        .collect();
    util::append(
        &wrapper,
        &components::tabs(ctx, "feed-tab", &tab_items, "feed_tab"),
    );

    // **The notice sits under the strip, above every pane.** It is the answer to
    // the last thing pressed, and the press that produces it is reachable from
    // all three — a refusal rendered inside one pane would be reported into a
    // pane nobody is looking at.
    if let Some(notice) = output.notice {
        util::append(&wrapper, &components::notice(&crate::i18n::t(notice.0, &[])));
    }

    // Matched rather than `_`-ed, so a fourth pane is a compile error here.
    match output.tab {
        FeedTab::Read => render_read(&wrapper, output, ctx),
        FeedTab::Yours => {
            let compose = util::create_element("div");
            let _ = compose.set_attribute("data-field", "feed-compose");
            render_composer(&compose, output, ctx);
            util::append(&wrapper, &compose);
        }
        FeedTab::Sources => {
            let manage = util::create_element("div");
            let _ = manage.set_attribute("data-field", "feed-manage");
            render_follow_form(&manage, output, ctx);
            render_follow_list(&manage, output, ctx);
            render_gatherers(&manage, output, ctx);
            util::append(&wrapper, &manage);
        }
    }

    util::append(container, &wrapper);
}

/// ⭐⭐ **The reading pane, which is TWO PAGES — the list of publishers, or one
/// publisher's posts.**
///
/// *"The site browser had it figured out: you navigate like a website."* It did,
/// and so does the Knowledge Base one window over — a list, an item, and a way
/// back — while this surface rendered the list and the archive stacked, so once
/// a publisher was on screen their posts were under everything forever and
/// picking a different one meant scrolling back up through them.
///
/// **The page is decided by the selection and by nothing else.** There is no
/// second bit saying *which page* — a bit that could disagree with the selection
/// is a bit that eventually does, and *back* is exactly *nobody is selected*
/// ([`crate::views::feed::model::FeedModel::back`]).
fn render_read(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    let pane = util::create_element("div");
    // The pane's own marker, so a gate can wait on *the reading surface is on
    // screen* without reading a caption or inferring it from what is inside.
    let _ = pane.set_attribute("data-field", "feed-read");

    match &output.selected {
        None => {
            // The list page. The hint describes **both** ways in and belongs
            // here rather than over every pane: it is advice about choosing a
            // publisher, which is the only thing this page is for.
            let hint = util::create_element("div");
            hint.set_attribute("style", theme::HINT).ok();
            util::set_text(&hint, &crate::i18n::t("feed.hint", &[]));
            util::append(&pane, &hint);
            render_known(&pane, output, ctx);
            render_nobody(&pane, output);
        }
        Some(selected) => render_panel(&pane, output, selected, ctx),
    }

    util::append(parent, &pane);
}

/// The nobody-selected line, under the list it is about.
///
/// ⛔ **Two states that pointed in opposite directions — TWO FACTS, not one.**
/// With no routed publishers the list says *"follow one by peer id below"* and
/// this used to say *"choose a publisher above"*, stacked one line apart on a
/// screen where there is nothing above. Each sentence is correct alone and no
/// test reads two of them together, which is why it took looking at the running
/// build. *Nobody has chosen* and *there is nobody to choose* get two sentences.
///
/// The marker is an attribute, not the wording: a control reading this screen
/// asserted `contains("Choose a publisher")` — an English copy string in a
/// thirty-locale app — and went red on the first rewording.
fn render_nobody(parent: &Element, output: &FeedOutput) {
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
}

/// The draft key for the composer box. Its own field for `GATHERER_FIELD`'s
/// reason — three boxes, three drafts, so no press can read what was typed into
/// a different one.
const POST_FIELD: &str = "feed_post";

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

/// **The composer** — post into your own tree, and unpublish what is there.
///
/// ⛔ **Its own pane, and not the one the window opens on.** The note that used
/// to sit here argued it belonged above the reading surface, because burying it
/// *"reads as an afterthought on a surface whose whole other half is reading"*.
/// The premise is what was wrong: this is not a window with two halves. Reading
/// what somebody else published and writing your own are two acts on two trees,
/// so they are two panes — and a pane, unlike the collapsed section this was
/// for a day, is not underneath whatever the other one is showing.
///
/// ⚠ **This doc comment was fused onto `place_body`'s** for as long as both
/// existed — two `///` blocks with the function between them lost, so `cargo
/// doc` attributed all of it to the wrong function. Worth knowing because it is
/// invisible to every gate here: prose is not checked, and a doc block that
/// slides onto its neighbour reads perfectly in the source.
///
/// ⛔ **`FEED-R21` lives on the Remove button's own notice, at the moment of the
/// action.** §7.5 is explicit that the honest sentence belongs there and not in
/// a help page, and the model hands it back from the verb
/// ([`crate::feed_compose::RemovalMeaning`]) so this renderer cannot forget to
/// ask for it — it renders whatever key the model chose, and for a removal that
/// key is the unpublication sentence.
fn render_composer(parent: &Element, output: &FeedOutput, ctx: &DomCtx) {
    // No heading of its own — the tab that opens this pane already carries
    // `feed.compose.heading`, and drawing it twice is the same word twice on
    // one screen.
    // ⭐⭐ **ONE DECISION, AND THE RENDERER ASKS FOR IT RATHER THAN INFERRING
    // IT.** `can_author` says whether the control is offered; `compose_note`
    // says what is written beside it. Two facts, so all four combinations are
    // expressible and only the model picks one — which is what makes the caveat
    // a caveat: *there is something to tell you* does not imply *and therefore
    // you may not*.
    //
    // ⚠ This block used to branch on *"is there a refusal key"*, so
    // `can_author()` had no consumer here at all and the surface re-derived the
    // decision from the presence of a sentence. Measured, not reasoned: a neuter
    // restoring the refusal to `AuthorKey::may_author` left the browser gate
    // green, because nothing the browser draws was reading it.
    let note = output.compose_note();
    if !output.can_author() {
        // **The one refusal, and it is about whose peer this is.** Not something
        // retrying or typing differently fixes, so offering the control would be
        // offering an act that cannot succeed.
        //
        // A dead box with nothing beside it is the thing to avoid on the way
        // past: every non-authoring arm carries a note
        // (`a_withheld_composer_always_says_why`), so the `None` arm here is
        // unreachable rather than a silent fallback.
        let el = components::empty(
            &note.map(|n| crate::i18n::t(n.key, &[])).unwrap_or_default(),
        );
        let _ = el.set_attribute("data-field", "feed-compose-unavailable");
        if let Some(n) = note {
            let _ = el.set_attribute("data-reason", n.reason);
        }
        util::append(parent, &el);
        return;
    }

    // **A caveat sits above the box; it does not replace it.** The
    // temporary-identity sentence was rendered by the refusal block for a day,
    // which turned *"this will not be saved"* into *"you may not do this"* — a
    // surface deciding, on somebody's behalf, that a thing which does not last
    // is not worth doing. The write into this peer's own tree is the publish
    // whatever happens to the tree afterwards; what is owed is the warning, and
    // it is owed **before** they type, which is why it is here and not in
    // `compose_notice` below.
    // ⭐ **And it is dismissible, because it is a standing fact rather than
    // news.** The sentence is owed once, before they type; drawing it over the
    // box on every render for the life of the tab is the surface repeating
    // itself at somebody who has already read it. Session-scoped, so a fresh
    // tab — which is a fresh temporary identity — is told again.
    if let Some(n) = output.compose_caveat() {
        let el = components::notice(&crate::i18n::t(n.key, &[]));
        let _ = el.set_attribute("data-field", "feed-compose-caveat");
        let _ = el.set_attribute("data-reason", n.reason);
        let dismiss = components::button_el(
            &crate::i18n::t("btn.dismiss", &[]),
            components::ButtonKind::Small,
        );
        let _ = dismiss.set_attribute("data-field", "feed-caveat-dismiss");
        {
            let actions = ctx.actions.clone();
            let rp = ctx.repaint.clone();
            let wid = output.window_id;
            ctx.listen(&dismiss, "click", move |_| {
                actions.borrow_mut().push(Action::WindowEvent {
                    window_id: wid,
                    event: "feed_dismiss_caveat".to_string(),
                    value: String::new(),
                });
                rp();
            });
        }
        util::append(&el, &dismiss);
        util::append(parent, &el);
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
        // The same date a reader of your feed sees, from the same expression —
        // your own timeline is the one place you would notice it disagreeing.
        let _ = id.set_attribute("data-created-at", &post.created_at.to_string());
        util::set_text(
            &id,
            &format!(
                "{} · {}",
                util::local_datetime(post.created_at), // i18n-ignore — platform-formatted
                post.id_short                          // i18n-ignore — a content hash
            ),
        );
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
/// This is the section that makes the window a reader instead of a text box.
/// The peer-id box is the **escape hatch** for a publisher we have no route to,
/// and a surface that leads with it tells an arriving visitor that the normal
/// way in is to paste 45 characters they do not have — so it lives under
/// *Manage sources*, one pane over.
///
/// ⭐ **Except when this list is empty, where it is rendered right here.** On a
/// profile with no routed publisher there is nothing on this page to choose
/// from, and *"follow one by peer id"* is then the only thing left to do — so
/// the box is put where the sentence saying that is, rather than on a pane the
/// visitor has no reason to look at. Both drafts are the same `PEER_FIELD`, and
/// only one of the two is ever on screen.
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
        // …and the box that sentence points at. See this function's doc: the
        // copy says *below*, so below is where it has to be when there is
        // nothing else on the page.
        render_follow_form(parent, output, ctx);
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
        // that reds the day somebody rewords it. `own`/`followed` are the two
        // facts a test needs and a person reads off the labels. **There was a
        // third, `data-home`**, and it is gone with the privilege it described.
        let _ = open.set_attribute("data-own", if row.own { "1" } else { "0" });
        let _ = open.set_attribute("data-followed", if row.followed { "1" } else { "0" });
        // **Which row the panel is reading, as a fact rather than a button
        // colour.** It was carried only by `ButtonKind::Primary`, so *"nobody
        // is being read"* — the property that replaced the home fallback — was
        // assertable only by matching a style.
        let _ = open.set_attribute("data-selected", if row.selected { "1" } else { "0" });
        ctx.on_window_event(&open, "click", "feed_select", &row.peer_id);

        // What this peer IS to this profile, in its own column. **One fact
        // now**, not two: the other was *this site's publisher*, which said
        // where a peer is hosted while reading as a standing they hold. An
        // empty cell is the ordinary case, which a column makes readable and a
        // trailing chip did not.
        let relation = if row.own { crate::i18n::t("feed.known.own", &[]) } else { String::new() };

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
                components::td(&publisher_cell(ctx, &row.peer_id, &row.label)),
                components::td_text(&relation),
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

/// ⭐ **One publisher in a list: what we call them, over the id that is
/// actually them.**
///
/// The word and the identifier are two different things a person needs and
/// neither replaces the other — which is the lesson `copy_code`'s own doc
/// records from the file-transfer table, where showing *only* a friendly name
/// left *"which peer is this"* unanswerable. So the id stays, in full and
/// copyable, and the name goes above it when there is one.
///
/// **[`PeerLabel::Origin`] draws here and not as a heading.** On this list it is
/// usually the only word available — these are the peers this deployment routes
/// to — and it is honest as a caption beside an id in a way it would not be as a
/// title claiming to name somebody.
///
/// One function, three call sites (browse row, follow row, and the panel's own
/// head reads the same `PeerLabel`), so two lists cannot render the same
/// publisher differently.
fn publisher_cell(
    ctx: &DomCtx,
    peer_id: &str,
    label: &crate::views::feed::output::PeerLabel,
) -> Element {
    use crate::views::feed::output::PeerLabel;
    let cell = util::create_element("div");
    let _ = cell.set_attribute("data-field", "feed-publisher");
    // The source as an attribute so a gate can assert *which* word won without
    // reading a translated one.
    let _ = cell.set_attribute("data-label-source", label.source());
    let word = match label {
        PeerLabel::Petname(s) | PeerLabel::Via(s) => Some(s.clone()),
        // Said as a routing fact, because that is what it is.
        PeerLabel::Origin(host) => Some(crate::i18n::t("feed.hosted_at", &[("origin", host)])),
        PeerLabel::Unnamed => None,
    };
    if let Some(word) = word {
        let name = util::create_element("div");
        let _ = name.set_attribute("data-field", "feed-publisher-name");
        util::set_text(&name, &word); // i18n-ignore — a petname, a resolved name, or an already-translated line
        util::append(&cell, &name);
    }
    util::append(&cell, &components::copy_code(ctx, peer_id, None));
    cell
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

        // ⭐ **The id is a VALUE, not a button caption** — the same fix the
        // browse table got on 2026-09-16, which this list had been left out of:
        // a 45-character identifier as a label cannot be read, and cannot be
        // selected to copy either, because clicking it selects the publisher.
        // `copy_code` carries it in full; the button says what pressing it does.
        // The name above it comes from the same `PeerLabel` the browse row and
        // the panel head read, so one publisher reads the same in all three.
        util::append(&line, &publisher_cell(ctx, &row.peer_id, &row.label));

        let open = components::button_el(
            &crate::i18n::t("feed.read", &[]),
            if row.selected {
                components::ButtonKind::Primary
            } else {
                components::ButtonKind::Secondary
            },
        );
        let _ = open.set_attribute("data-field", "feed-open");
        let _ = open.set_attribute("data-peer", &row.peer_id);
        // Pressing it moves to the reading pane — `FeedModel::select` owns that,
        // because a press here that only changed a pane nobody is looking at is
        // the dead-button disease.
        ctx.on_window_event(&open, "click", "feed_select", &row.peer_id);
        util::append(&line, &open);

        let drop = components::button_el(
            &crate::i18n::t("feed.unfollow", &[]),
            components::ButtonKind::Secondary,
        );
        let _ = drop.set_attribute("data-field", "feed-unfollow");
        ctx.on_window_event(&drop, "click", "feed_unfollow", &row.peer_id);
        util::append(&line, &drop);

        // ⭐⭐ **The petname box — §2.4's `label`, and it is only here.**
        //
        // *"A petname: local, chosen by the reader, and never authoritative …
        // the answer to 'I cannot read a public key' that requires no naming
        // authority at all."* The convention shipped the field, this crate
        // shipped the codec, and nothing ever wrote one — so a surface whose
        // whole complaint was *"all you show me is the key"* had the remedy in
        // its own data model, unwired.
        //
        // ⛔ **Drawn only in this list, and that is what makes
        // `LabelOutcome::NotFollowing` unreachable rather than merely
        // handled**: the name hangs off the follow record, so a box over a
        // publisher you do not follow would be a control whose only outcome is
        // a refusal — the same defect the browse table's Follow-on-your-own-row
        // was.
        //
        // **Per-row draft key**, or every row in the list would share one box's
        // contents (`text_input` tracks drafts by key, which is what makes
        // typing survive a rebuild) and naming one publisher would pre-fill the
        // name of the next.
        let draft_key = format!("feed_alias_{}", row.peer_id);
        let alias = components::text_input(
            ctx,
            &draft_key,
            // **The stored petname, never the rendered label** — which may have
            // fallen through to a resolved name or a host. A box pre-filled with
            // something the person did not type turns a `via` into a petname on
            // the first Save.
            row.petname.as_deref().unwrap_or_default(),
            &crate::i18n::t("feed.alias_placeholder", &[]),
        );
        let _ = alias.set_attribute("data-field", "feed-alias");
        let _ = alias.set_attribute("data-peer", &row.peer_id);
        util::append(&line, &alias);

        let save = components::button_el(
            &crate::i18n::t("btn.save", &[]),
            components::ButtonKind::Secondary,
        );
        let _ = save.set_attribute("data-field", "feed-alias-save");
        // `{peer}\x1f{name}` — a unit separator, so an EMPTY name survives as an
        // empty second field. That is the **clear**, and a whitespace split
        // would make an alias impossible to remove.
        //
        // The draft is read at press time from the same key the box writes,
        // rather than travelling on the event: `on_window_event` takes a static
        // value and the box's contents change under it.
        {
            let actions = ctx.actions.clone();
            let rp = ctx.repaint.clone();
            let drafts = ctx.drafts.clone();
            let peer = row.peer_id.clone();
            let wid = output.window_id;
            let key = draft_key.clone();
            ctx.listen(&save, "click", move |_| {
                let typed = drafts.borrow().get(&key).cloned().unwrap_or_default();
                actions.borrow_mut().push(Action::WindowEvent {
                    window_id: wid,
                    event: "feed_set_label".to_string(),
                    value: format!("{peer}\u{1f}{typed}"),
                });
                rp();
            });
        }
        util::append(&line, &save);

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

/// One publisher's posts — the reading pane's second page.
fn render_panel(parent: &Element, output: &FeedOutput, selected: &Selection, ctx: &DomCtx) {
    let head = util::create_element("div");
    head.set_attribute("style", theme::ROW_INLINE).ok();

    // ⭐ **The way out, and it leads the row.** Without it the only route back
    // to the list of publishers was scrolling to the top of somebody's archive
    // — which is the Knowledge Base's `back_to_list` and the Site Browser's
    // navigation, both of which this window had and used to lack. `btn.back`
    // rather than a `feed.*` key of its own: one English word, one key.
    let back = components::button_el(
        &crate::i18n::t("btn.back", &[]),
        components::ButtonKind::Secondary,
    );
    let _ = back.set_attribute("data-field", "feed-back");
    ctx.on_window_event(&back, "click", "feed_back", "");
    util::append(&head, &back);
    // ⭐ **A heading that says something, and the id in full underneath.** This
    // was `subheading(&peer_id)` — a 45-character identifier as the section
    // title, which tells a person nothing and sets the width of the window.
    // The heading now carries the one true thing we know about this publisher
    // (*you* / *this deployment's publisher* / *publisher*); the id follows in
    // `copy_code`, complete and copyable. See
    // [`crate::views::feed::output::Selection::heading_key`] for why there is no
    // name to put there and why that is a held decision rather than a gap here.
    //
    // ⭐⭐ **…and it is the publisher's NAME when we have one.** Everything
    // above was written when this surface had nothing but a key; §2.4's
    // petname and `via` are the two words a reader can legitimately have for
    // somebody, and both were implemented in the codec and wired to nothing.
    // The generic word is now the fallback rather than the only answer.
    //
    // `PeerLabel::Origin` is deliberately **not** a heading — `name()` refuses
    // to hand a hostname back for exactly this reason. Where they are hosted is
    // a routing fact and gets its own line below, phrased as one.
    let heading = match selected.label.name() {
        Some(name) => name.to_string(),
        None => crate::i18n::t(selected.heading_key(), &[]),
    };
    let title = components::subheading(&heading);
    // The word, and where it came from, as attributes — never as a sentence.
    // A gate reading a translated heading reds the day it is reworded, which
    // this window has already paid for once.
    let _ = title.set_attribute("data-field", "feed-panel-title");
    let _ = title.set_attribute("data-label-source", selected.label.source());
    util::append(&head, &title);
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

    // ⭐ **Where their bytes come from — a routing fact, said as one.**
    //
    // It is the only thing this deployment knows about every publisher it can
    // reach, and on a multi-domain estate it is the one line that tells six
    // otherwise-identical keys apart. **Same-origin (`Some("")`) draws nothing**
    // rather than `Hosted at ` — a recorded empty origin is a real value and
    // still not a host, which is `peer_label`'s rule arriving at the pixel.
    if let Some(host) = selected
        .origin
        .as_deref()
        .and_then(|o| (!o.trim().is_empty()).then_some(o))
    {
        let hosted = util::create_element("div");
        hosted.set_attribute("style", theme::HINT).ok();
        let _ = hosted.set_attribute("data-field", "feed-panel-origin");
        util::set_text(&hosted, &crate::i18n::t("feed.hosted_at", &[("origin", host)]));
        util::append(parent, &hosted);
    }

    // ⭐ **What following does, at the control rather than in a help page.** The
    // question *"I clicked follow — what does that mean?"* had no answer
    // anywhere on this surface. A follow writes one row into **your own** tree;
    // its whole effect is that this publisher joins the set the panel reads on
    // each refresh, over whichever leg is live for them. It notifies nobody,
    // subscribes to nothing, and pulls nothing in the background — and a person
    // who assumes otherwise is owed the correction here, where they are about
    // to press it.
    //
    // ⭐ **Only where the button it explains says *Follow*.** It rendered above
    // every publisher including the ones already followed, so a paragraph
    // answering a question somebody asked once sat over their feed forever —
    // three lines of prose between the heading and the posts, on the surface
    // whose complaint was that the reading is buried. *An explanation belongs
    // beside the decision, and after the decision it is clutter.*
    if crate::views::feed::output::relation(selected.own, selected.followed)
        == crate::views::feed::output::Relation::Stranger
    {
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
        FeedPanel::Entries { via, rows } => render_entries(parent, via, rows, ctx),
    }
}

/// The draft key for the filter box. Its own field, like the other three.
const FILTER_FIELD: &str = "feed_filter";

/// ⭐ **How many posts it takes before a filter is worth offering.**
///
/// *"What is this filter going to do — filter what?"* A bare text box above four
/// cards that are all on screen at once cannot help with anything, so what it
/// mostly does is raise that question. Above this many the list is longer than a
/// screen and narrowing it is a real act.
///
/// **Not a magic number in a condition**: the pair either side of it is gated —
/// the gathered rig carries three posts and must show no filter, the published
/// one carries 34 and must — so moving it is a decision with two assertions on
/// it rather than a taste.
const FILTER_FROM: usize = 8;

/// ⭐⭐ **How many posts a reading pane opens with.**
///
/// The complaint this exists for: a reading pane that scrolled all the way to
/// the end with no paging and no count -- a publisher's whole archive rendered
/// as one unbroken column, with no number anywhere saying how much of it there
/// was.
///
/// ⛔ **This is a READING cap and it is not the fetch's.** The two are different
/// dimensions and conflating them is how *"show me more"* turns into a network
/// round trip nobody asked for: [`crate::feed_fetch::LIMIT`] bounds what one
/// walk **obtains**, this bounds what one screen **draws**, and every post
/// counted here is already in hand. Revealing more is free and instant, which
/// is what makes it a DOM control rather than an event through the model.
///
/// Above [`FILTER_FROM`] so that any archive long enough to be capped is also
/// long enough to have a filter — a person told there are more posts should have
/// the means to find one.
const PAGE_SIZE: usize = 20;

/// ⭐⭐ **Show only the posts that match, and do it in the DOM.**
///
/// The one thing this must not do is round-trip through the model. A filter
/// that dispatched a `WindowEvent` per keystroke would rebuild the window under
/// the caret on every letter, and the draft machinery that makes typing survive
/// a rebuild restores the *value*, not the focus — so the box would empty itself
/// of attention after one character. Hiding rows that are already on screen
/// changes no state, marks nothing dirty and cannot race a landing walk.
///
/// It is called from **two** places and that is what makes it correct rather
/// than a trick: from the input listener on every keystroke, and from
/// [`render_entries`] with whatever the draft already holds — so a rebuild (a
/// refresh landing, a follow, a repaint from anywhere) re-applies the filter
/// instead of silently dropping it. One predicate, over the same `textContent`,
/// both times; a render-time filter written in Rust over `EntryRow` would be a
/// *second* predicate that could disagree with the live one about the same post.
///
/// ⛔ **A filter that hides everything says so.** An empty list under a box
/// somebody has typed into is indistinguishable from a publisher who posted
/// nothing — the collapse `FeedPanel::Loading`/`NoPosts` exists one layer down
/// to prevent, arriving here by a different road.
///
/// ⭐⭐ **It also applies the READING CAP, and the two are one pass on purpose.**
/// Filtering and capping are both *"which of these slots are on screen"*, and
/// two functions each writing `display` on the same elements is two predicates
/// that disagree the first time they run in the wrong order — the exact reason
/// the filter is one predicate over `textContent` rather than a DOM filter plus
/// a Rust one. The cap counts **matches**, not slots, so narrowing a long
/// archive to three posts shows three rather than three out of the first
/// twenty.
///
/// Returns how many matched in total, so the caller can say whether the cap is
/// hiding anything — *a control that reveals more must not appear when there is
/// no more*, and a count is the only way to know.
fn apply_view(list: &Element, empty: &Element, needle: &str, cap: usize) -> usize {
    let needle = needle.trim().to_lowercase();
    let slots = list.query_selector_all("[data-field=\"feed-entry-slot\"]").ok();
    let mut matched = 0usize;
    let mut shown = 0usize;
    if let Some(slots) = slots {
        for i in 0..slots.length() {
            let Some(slot) = slots.item(i).and_then(|n| n.dyn_into::<Element>().ok()) else {
                continue;
            };
            let hit = needle.is_empty()
                || slot
                    .text_content()
                    .map(|t| t.to_lowercase().contains(&needle))
                    .unwrap_or(false);
            if hit {
                matched += 1;
            }
            let visible = hit && matched <= cap;
            // The slot exists so that hiding a card never touches the card's own
            // `style` — `components::card` puts the whole look there, and an
            // un-hide that wrote `display:block` over it would return a
            // different-looking post than the one that went away.
            let _ = slot.set_attribute("style", if visible { "" } else { "display:none" });
            if visible {
                shown += 1;
            }
        }
    }
    // ⚠ **`data-shown` is what the DOM DID; `data-matched` is what the model
    // would have said.** A gate reading only the second measures the decision
    // and not the effect — which is how a hide that never happened passes a
    // check on the surface's own count.
    let _ = list.set_attribute("data-shown", &shown.to_string());
    let _ = list.set_attribute("data-matched", &matched.to_string());
    let _ = empty.set_attribute("style", if shown == 0 { theme::HINT } else { "display:none" });
    matched
}

fn render_entries(parent: &Element, via: &Via, rows: &[EntryRow], ctx: &DomCtx) {
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

    // ⭐ **How many posts this is.** The pane had no number anywhere, so
    // *"how much of this is there?"* had no answer and the scroll bar was the
    // only evidence — which is what made a working index walk read as broken
    // paging. It also gives Refresh something observable to change.
    let count = util::create_element("div");
    count.set_attribute("style", theme::HINT).ok();
    let _ = count.set_attribute("data-field", "feed-count");
    let _ = count.set_attribute("data-count", &rows.len().to_string());
    // ⚠ **`t_plural` picks the FORM; it does not fill the slot.** Passing `&[]`
    // renders *"<FSI><PDI> posts"* — the bidi isolation marks around an empty
    // substitution — and it is green in every native test, because nothing
    // native reads this line. Found by reading a browser gate's failure dump for
    // an unrelated neuter. The count goes in twice on purpose: once to choose
    // the plural rule, once as the value.
    util::set_text(
        &count,
        &crate::i18n::t_plural(
            "feed.post_count",
            rows.len() as i64,
            &[("n", &rows.len().to_string())],
        ),
    );
    util::append(parent, &count);

    // ⛔⭐ **AND WHETHER THAT IS ALL OF THEM — the honest hedge.**
    //
    // One walk takes at most `feed_fetch::LIMIT` entries. When it comes back
    // exactly full we **cannot tell** a publisher who has that many from one who
    // has more: `read_feed_from` computes `truncated` and the no-cursor arm
    // discards it (`Resumed::FromNewest` is returned either way), so the fact
    // exists one module down and reaches no caller. That is the same defect
    // `NameListing::complete` exists to prevent one window over — *a shortened
    // list that does not announce itself* — and until the extent is threaded
    // through `feed_route::reduce` this is what can honestly be said.
    //
    // So the sentence says *there may be* and the condition is our own request
    // bound rather than a guess about the publisher. Unreachable on every feed
    // published today (the largest is 34 against a limit of 50), which is
    // exactly why it would otherwise ship untested and wrong.
    if rows.len() >= crate::feed_fetch::LIMIT {
        let more = util::create_element("div");
        more.set_attribute("style", theme::HINT).ok();
        let _ = more.set_attribute("data-field", "feed-newest-only");
        util::set_text(
            &more,
            &crate::i18n::t(
                "feed.newest_only",
                &[("n", &crate::feed_fetch::LIMIT.to_string())],
            ),
        );
        util::append(parent, &more);
    }

    // **The filter, beside the line that says where these came from** — and
    // only on an archive long enough for narrowing to mean anything
    // ([`FILTER_FROM`]). It sits above the posts because that is where a person
    // looks for it, and because a control below a hundred cards is a control
    // nobody finds.
    let filter = (rows.len() > FILTER_FROM).then(|| {
        let filter_row = util::create_element("div");
        filter_row.set_attribute("style", theme::ROW_INLINE).ok();
        let filter = components::text_input(
            ctx,
            FILTER_FIELD,
            "",
            &crate::i18n::t("feed.filter_placeholder", &[]),
        );
        let _ = filter.set_attribute("data-field", "feed-filter");
        util::append(&filter_row, &filter);
        util::append(parent, &filter_row);
        filter
    });

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
        // The slot the filter hides. See `apply_filter` for why the card's own
        // `style` is not the thing toggled.
        let slot = util::create_element("div");
        let _ = slot.set_attribute("data-field", "feed-entry-slot");

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
        // ⭐ **WHEN IT WAS WRITTEN, AND IT LEADS.** `created_at` has been on
        // `EntryRow` since the type existed and reached no screen — every post
        // rendered as body plus a hash plus a verdict, with no date anywhere, on
        // a surface whose whole subject is a timeline. It is the author's own
        // claim, rendered verbatim by the platform: `FEED-R8` forbids rejecting
        // an entry for an implausible timestamp, and quietly re-writing one we
        // find unlikely is that refusal wearing a formatter's clothes.
        //
        // **The verdict is rendered for every entry, including the good one.** A
        // surface that showed a chip only when something was wrong would make
        // *"this is verified"* indistinguishable from *"nobody looked"*.
        let _ = meta.set_attribute("data-created-at", &row.created_at.to_string());
        util::set_text(
            &meta,
            &format!(
                "{} · {} · {}",
                util::local_datetime(row.created_at), // i18n-ignore — platform-formatted
                row.id_short,                         // i18n-ignore — a content hash
                crate::i18n::t(row.attribution_key, &[])
            ),
        );
        util::append(&card, &meta);

        util::append(&slot, &card);
        util::append(&list, &slot);
    }
    util::append(parent, &list);

    let empty = util::create_element("div");
    let _ = empty.set_attribute("data-field", "feed-filter-empty");
    util::set_text(&empty, &crate::i18n::t("feed.filter_no_match", &[]));
    util::append(parent, &empty);

    // Apply whatever is already typed, **before** wiring the listener: this is
    // the rebuild path, and a filter that only ran on keystrokes would come back
    // showing every post with its own box still full.
    //
    // ⚠ **And it runs even when no box was drawn**, which is the case a tidy
    // version would put inside the `if`: the draft survives navigation, so
    // somebody who filtered a long archive, went Back and opened a short one
    // would otherwise be looking at a list narrowed by a word they can no longer
    // see or clear. An empty needle shows everything, so the short archive comes
    // back whole — and the *"nothing matches"* line, which this call also
    // decides, is drawn either way.
    let typed = match &filter {
        Some(_) => ctx.drafts.borrow().get(FILTER_FIELD).cloned().unwrap_or_default(),
        None => String::new(),
    };

    // ⭐ **The reveal control — and the cap it lifts lives in the DOM, not in
    // the model.**
    //
    // Everything it reveals is already on the page; pressing it changes no
    // state, marks nothing dirty and cannot race a landing walk — the same
    // argument the filter is built on, and the reason neither goes through a
    // `WindowEvent`. A round trip through the model would rebuild the window
    // under the caret and empty the filter box of attention on every press.
    //
    // ⚠ **The cap is session-scoped to this render**, so a repaint (a refresh
    // landing, a follow) returns to the first page. That is deliberate rather
    // than an oversight: a rebuild re-renders whatever the walk now holds, and
    // carrying a reveal count across it would show a person the top of a list
    // they had scrolled past with no way to tell what moved.
    let cap = std::rc::Rc::new(std::cell::Cell::new(PAGE_SIZE));
    let more = components::button_el(
        // `contentsite.more` — *"More ▾ ({n})"*, already in thirty locales and
        // already meaning exactly this. One English phrase, one key.
        &crate::i18n::t("contentsite.more", &[("n", "0")]),
        components::ButtonKind::Secondary,
    );
    let _ = more.set_attribute("data-field", "feed-more");
    util::append(parent, &more);

    // One function refreshes both the visibility and the control that governs
    // it — **a reveal button that outlived its own reason is the dead-button
    // disease**, and the count it carries is what tells somebody whether
    // pressing it again will do anything.
    let refresh_view = {
        let list = list.clone();
        let empty = empty.clone();
        let more = more.clone();
        let cap = cap.clone();
        std::rc::Rc::new(move |needle: &str| {
            let matched = apply_view(&list, &empty, needle, cap.get());
            let remaining = matched.saturating_sub(cap.get());
            let _ = more.set_attribute("data-remaining", &remaining.to_string());
            let _ = more.set_attribute(
                "style",
                if remaining == 0 { "display:none" } else { "" },
            );
            more.set_text_content(Some(&crate::i18n::t(
                "contentsite.more",
                &[("n", &remaining.to_string())],
            )));
        })
    };
    refresh_view(&typed);

    {
        let cap = cap.clone();
        let refresh_view = refresh_view.clone();
        let drafts = ctx.drafts.clone();
        ctx.listen(&more, "click", move |_| {
            cap.set(cap.get() + PAGE_SIZE);
            // **Re-read the draft rather than capturing it**: the box's contents
            // change under this closure, and a reveal that re-applied a stale
            // needle would show posts the filter had just excluded.
            let needle = drafts.borrow().get(FILTER_FIELD).cloned().unwrap_or_default();
            refresh_view(&needle);
        });
    }

    if let Some(filter) = filter {
        let refresh_view = refresh_view.clone();
        ctx.listen(&filter, "input", move |evt: web_sys::Event| {
            let value = evt
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|i| i.value())
                .unwrap_or_default();
            refresh_view(&value);
        });
    }
}

//! Registry Browser DOM renderer — a pure consumer of
//! [`RegistryBrowserOutput`](crate::views::registry_browser::output::RegistryBrowserOutput).
//!
//! **Two honesty rules are enforced here rather than left to a caller**, because
//! this is the last place before pixels:
//!
//! 1. A **truncated** listing renders its truncation notice. A short list that
//!    does not announce itself is indistinguishable from a small registry — the
//!    same defect the signed walk itself exists to prevent, one layer up.
//! 2. **`Idle` is not `Done(empty)`.** "Nothing fetched yet" and "walked the
//!    whole root and found nothing" are different sentences, and rendering them
//!    identically tells a person a registry is empty when nothing was asked.

use web_sys::Element;

use crate::dom::components as c;
use crate::dom::util::{self, DomCtx};
use crate::views::registry_browser::output::{
    NameListing, Phase, PinOrigin, Publications, RegistryBrowserOutput, ResolvedName,
};
use crate::window::WindowId;

const NAME_FIELD: &str = "registry_name";
const PIN_PID_FIELD: &str = "registry_pin_pid";
const PIN_ORIGIN_FIELD: &str = "registry_pin_origin";

pub fn render(
    container: &Element,
    output: &RegistryBrowserOutput,
    ctx: &DomCtx,
    window_id: WindowId,
) {
    // No inline style on the wrapper: `card` already owns its own margin and
    // padding, so a container style here would be a second source for the same
    // spacing decision (and the ui-lint ratchet is what caught it).
    let root = util::create_element("div");

    render_pin(&root, output, ctx);
    render_names(&root, output, ctx);
    render_resolve(&root, output, ctx, window_id, &output.local_peer);

    container.set_inner_html("");
    util::append(container, &root);
}

/// Which registry is in force, and **where it came from**.
///
/// The source line is not decoration [AP25]: a deployment-seeded pin means names
/// resolve through a registry the user never typed. That is the feature, and it
/// is exactly the thing that must not be silent.
fn render_pin(root: &Element, output: &RegistryBrowserOutput, ctx: &DomCtx) {
    let card = c::card(&crate::i18n::t("registry.pinned", &[]));
    match &output.pinned {
        Some(p) => {
            let (t, tbody) = c::table(&[
                &crate::i18n::t("label.peer_id", &[]),
                &crate::i18n::t("registry.origin", &[]),
                &crate::i18n::t("registry.source", &[]),
            ]);
            let source = match p.source {
                PinOrigin::User => crate::i18n::t("registry.source_user", &[]),
                PinOrigin::Deployment => crate::i18n::t("registry.source_deployment", &[]),
            };
            let origin = if p.origin.is_empty() {
                crate::i18n::t("registry.this_origin", &[])
            } else {
                p.origin.clone()
            };
            util::append(
                &tbody,
                &c::tr(vec![
                    c::td_text(&crate::views::short_pid(&p.peer_id)),
                    c::td_text(&origin),
                    c::td_text(&source),
                ]),
            );
            util::append(&card, &t);
        }
        // **Fail-closed, and say so.** No pin means no resolution at all; a blank
        // panel here would read as "this registry is empty".
        None => util::append(&card, &c::notice(&crate::i18n::t("registry.no_pin", &[]))),
    }

    // The form. Until 2026-08-21 this window could only *report* a pin —
    // `pinned()` read the deployment seed and nothing else, and the only way to
    // set one was `name pin` in the Shell, which wrote a `Mutex` on that window
    // that this one could not see. So "no registry pinned" was a dead end with
    // no affordance anywhere in the GUI. Both surfaces write one shared pin now.
    render_pin_form(&card, output, ctx);

    util::append(
        &card,
        &c::pre_notice(&crate::i18n::t(
            "registry.sessions",
            &[("n", &output.sessions.to_string())],
        )),
    );
    util::append(root, &card);
}

/// Two fields and a button: the peer-id (the trust decision) and the origin
/// (merely where to fetch).
///
/// **The origin may be left empty** — that means same-origin, exactly as in the
/// deployment's `origins` map, and it is the common case when the registry is
/// published beside the app. The peer-id may not: pinning an origin would trust
/// the origin, which is the one thing this chain never does. Both refusals come
/// back from the model as text, so the reason is on screen rather than being a
/// button that does nothing.
fn render_pin_form(card: &Element, output: &RegistryBrowserOutput, ctx: &DomCtx) {
    let pid = c::text_input(ctx, PIN_PID_FIELD, "", &crate::i18n::t("registry.pin_pid_ph", &[]));
    util::append(card, &c::field(&crate::i18n::t("label.peer_id", &[]), "", &pid));

    let origin = c::text_input(
        ctx,
        PIN_ORIGIN_FIELD,
        "",
        &crate::i18n::t("registry.pin_origin_ph", &[]),
    );
    util::append(card, &c::field(&crate::i18n::t("registry.origin", &[]), "", &origin));

    // Read at click time, not per keystroke — a tree write per character
    // rebuilds the snapshot and destroys focus (same rule as the resolve field).
    let go = c::button_el(&crate::i18n::t("registry.pin_btn", &[]), c::ButtonKind::Primary);
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let card_ref = card.clone();
        ctx.listen(&go, "click", move |_| {
            let read = |field: &str| {
                card_ref
                    .query_selector(&format!("[data-field='{field}']"))
                    .ok()
                    .flatten()
                    .and_then(|el| {
                        wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlInputElement>(el).ok()
                    })
                    .map(|i| i.value())
                    .unwrap_or_default()
            };
            let peer_id = read(PIN_PID_FIELD);
            if peer_id.trim().is_empty() {
                return;
            }
            // `\x1f`-packed, the repo's idiom for a multi-field save: an origin
            // cannot contain it, and splitting on it keeps an empty origin
            // (same-origin) distinguishable from an absent second field.
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id: wid,
                event: "registry_pin".into(),
                value: format!("{peer_id}\u{1f}{}", read(PIN_ORIGIN_FIELD)),
            });
            rp();
        });
    }
    util::append(card, &go);

    // Only offered when there is something of *yours* to drop. A deployment's
    // seed is not the user's to unpin from here — that is the deployment's
    // posture, and an "Unpin" that silently did nothing would be worse than none.
    if matches!(output.pinned.as_ref().map(|p| p.source), Some(PinOrigin::User)) {
        util::append(
            card,
            &c::button(
                ctx,
                &crate::i18n::t("registry.unpin_btn", &[]),
                c::ButtonKind::Small,
                "registry_unpin",
            ),
        );
    }

    if let Some(err) = &output.pin_error {
        util::append(card, &c::pre_notice(err));
    }
}

fn render_names(root: &Element, output: &RegistryBrowserOutput, ctx: &DomCtx) {
    let card = c::card(&crate::i18n::t("registry.names", &[]));

    let browse = c::button(
        ctx,
        &crate::i18n::t("registry.browse", &[]),
        c::ButtonKind::Primary,
        "registry_browse",
    );
    if output.pinned.is_none() || output.browser_only {
        c::disable(&browse);
    }
    util::append(&card, &browse);

    match &output.listing {
        // Distinct from `Done(empty)` on purpose — see the module docs.
        Phase::Idle => util::append(
            &card,
            &c::empty(&crate::i18n::t("registry.names_idle", &[])),
        ),
        Phase::Running => util::append(
            &card,
            &c::loading(&crate::i18n::t("registry.walking", &[])),
        ),
        // A failed walk is NOT an empty registry. `IncompleteWalk` in particular
        // is a statement about the origin — it withheld something its own signed
        // root declares — and rendering it as "no names" would launder a hostile
        // origin into a small one.
        Phase::Failed(e) => util::append(&card, &c::pre_notice(e)),
        Phase::Done(listing) => render_listing(&card, listing, ctx),
    }
    util::append(root, &card);
}

fn render_listing(card: &Element, listing: &NameListing, ctx: &DomCtx) {
    // The provenance line, always: these names came out of the signed root, not
    // out of the host-served `by-name.list`, and that difference is the whole
    // reason this panel is trustworthy at all.
    util::append(
        card,
        &c::pre_notice(&crate::i18n::t(
            "registry.from_signed_root",
            &[
                ("n", &listing.names.len().to_string()),
                ("nodes", &listing.nodes_walked.to_string()),
            ],
        )),
    );
    // **Rule 1.** Truncation is announced or the list is a lie by omission.
    if !listing.complete {
        util::append(card, &c::notice(&crate::i18n::t("registry.truncated", &[])));
    }
    if listing.names.is_empty() {
        util::append(
            card,
            &c::empty(&crate::i18n::t("registry.names_none", &[])),
        );
        return;
    }
    let (t, tbody) = c::table(&[&crate::i18n::t("label.name", &[]), ""]);
    for name in &listing.names {
        let resolve = c::button_value(
            ctx,
            &crate::i18n::t("registry.resolve", &[]),
            c::ButtonKind::Secondary,
            "registry_resolve",
            name,
        );
        util::append(&tbody, &c::tr(vec![c::td_text(name), c::td(&resolve)]));
    }
    util::append(card, &t);
}

fn render_resolve(
    root: &Element,
    output: &RegistryBrowserOutput,
    ctx: &DomCtx,
    _window_id: WindowId,
    local_peer: &str,
) {
    let card = c::card(&crate::i18n::t("registry.resolve_a_name", &[]));

    let input = c::text_input(ctx, NAME_FIELD, "", &crate::i18n::t("registry.name_ph", &[]));
    util::append(
        &card,
        &c::field(&crate::i18n::t("label.name", &[]), "", &input),
    );

    // Read the field at click time rather than tracking each keystroke: a
    // per-keystroke tree write would rebuild the snapshot and destroy focus.
    let go = c::button_el(&crate::i18n::t("registry.resolve", &[]), c::ButtonKind::Primary);
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let card_ref = card.clone();
        ctx.listen(&go, "click", move |_| {
            let value = card_ref
                .query_selector(&format!("[data-field='{NAME_FIELD}']"))
                .ok()
                .flatten()
                .and_then(|el| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlInputElement>(el).ok())
                .map(|i| i.value())
                .unwrap_or_default();
            if value.trim().is_empty() {
                return;
            }
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id: wid,
                event: "registry_resolve".into(),
                value,
            });
            rp();
        });
    }
    util::append(&card, &go);

    match &output.resolved {
        Phase::Idle => {}
        Phase::Running => util::append(
            &card,
            &c::loading(&crate::i18n::t("registry.resolving", &[])),
        ),
        Phase::Failed(e) => util::append(&card, &c::pre_notice(e)),
        Phase::Done(target) => render_evidence(
            &card,
            target,
            ctx,
            local_peer,
            &output.resolved,
            &output.published,
        ),
    }
    util::append(root, &card);
}

/// What the resolve **established**, and what it **checked** to establish it.
///
/// The checks are shown individually rather than collapsed into a tick, because
/// `NameEvidence` records which ones actually ran and a resolved name must not
/// look like an unchecked one. `expires_at_ms` is shown as a date for the same
/// reason `GUIDE-SERVING-MODE` §8 requires one on "Verified": a lifetime with no
/// endpoint reads as permanent.
fn render_evidence(
    card: &Element,
    target: &ResolvedName,
    ctx: &DomCtx,
    local_peer: &str,
    // The same phase `target` came out of. `opens` takes the PHASE rather than
    // the resolved name so its "nothing resolved yet" guard stays in natively
    // tested code instead of moving into this wasm-only file.
    resolved: &Phase<ResolvedName>,
    published: &Phase<Publications>,
) {
    let (t, tbody) = c::table(&[&crate::i18n::t("registry.checked", &[]), ""]);
    let mut row = |k: &str, v: String| {
        util::append(&tbody, &c::tr(vec![c::td_text(k), c::td_text(&v)]));
    };
    row(&crate::i18n::t("label.name", &[]), target.name.clone());
    row(
        &crate::i18n::t("label.peer_id", &[]),
        crate::views::short_pid(&target.peer_id),
    );
    row(
        &crate::i18n::t("registry.origin", &[]),
        target
            .origin
            .clone()
            .unwrap_or_else(|| crate::i18n::t("registry.no_origin", &[])),
    );
    let yn = |b: bool| if b { "\u{2713}" } else { "\u{2717}" }.to_string();
    row(
        &crate::i18n::t("registry.association", &[]),
        yn(target.association_committed),
    );
    row(&crate::i18n::t("registry.name_match", &[]), yn(target.name_checked));
    row(
        &crate::i18n::t("registry.revocation", &[]),
        yn(target.revocation_checked),
    );
    row(
        &crate::i18n::t("registry.expires", &[]),
        crate::views::format_day(target.expires_at_ms),
    );
    util::append(card, &t);

    // Say it when OUR ceiling is why the expiry is sooner than the registry said
    // [AP25] — a silent clamp makes a correctly-issued binding look like a
    // registry that mis-set its TTL.
    if let Some((issued, honored)) = target.clamped {
        util::append(
            card,
            &c::pre_notice(&crate::i18n::t(
                "registry.clamped",
                &[("issued", &issued.to_string()), ("honored", &honored.to_string())],
            )),
        );
    }

    render_publications(card, resolved, ctx, local_peer, published);
}

/// ⭐ **What the resolved publisher actually carries — and every refusal is a
/// row, not a silence.**
///
/// A resolve establishes *who* and *where*; a `system/registry/binding` says
/// nothing about *what*. This asks the publisher's own signed root
/// ([`crate::publication_probe`]) and renders **one row per registered
/// convention**, including the ones that answered no and the ones that could
/// not be asked.
///
/// **The two rules this enforces, both of which are the module's existing pair
/// pointed at a new subject:**
///
/// 1. **Not probed yet is not "they publish nothing".** `Idle` gets its own
///    sentence, exactly as the names listing does.
/// 2. **Only a demonstrated convention gets a button.** The retired
///    `open_target` offered a Site Browser for every resolved name, so a
///    feed-only publisher got a window whose rail is empty by construction —
///    a correct registry answer rendered as *"this publisher has nothing"*.
fn render_publications(
    card: &Element,
    resolved: &Phase<ResolvedName>,
    ctx: &DomCtx,
    local_peer: &str,
    published: &Phase<Publications>,
) {
    use crate::publication_probe::{Publishes, Unknown};

    match published {
        // Not asked yet — and a blank here would read as an answer.
        Phase::Idle => {
            util::append(card, &c::empty(&crate::i18n::t("registry.published_idle", &[])));
            return;
        }
        Phase::Running => {
            util::append(card, &c::loading(&crate::i18n::t("registry.probing", &[])));
            return;
        }
        Phase::Failed(e) => {
            util::append(card, &c::pre_notice(e));
            return;
        }
        Phase::Done(found) => {
            let (t, tbody) = c::table(&[&crate::i18n::t("registry.published", &[]), ""]);
            for f in found {
                let said = match &f.outcome {
                    Publishes::Yes { units: Some(n) } => {
                        crate::i18n::t("registry.pub_yes_n", &[("n", &n.to_string())])
                    }
                    Publishes::Yes { units: None } => crate::i18n::t("registry.pub_yes", &[]),
                    Publishes::No => crate::i18n::t("registry.pub_no", &[]),
                    Publishes::Partial { nodes_walked } => crate::i18n::t(
                        "registry.pub_partial",
                        &[("nodes", &nodes_walked.to_string())],
                    ),
                    Publishes::Unknown(u) => crate::i18n::t(
                        match u {
                            Unknown::Unreachable(_) => "registry.pub_unreachable",
                            Unknown::Withheld(_) => "registry.pub_withheld",
                            Unknown::Unproven(_) => "registry.pub_unproven",
                            Unknown::OurFloor(_) => "registry.pub_declined",
                            Unknown::Exhausted => "registry.pub_exhausted",
                        },
                        &[],
                    ),
                };
                // The window type is an identity key AND the viewer's name on
                // screen — the same string `window_registry` carries, so a row
                // here cannot name a viewer the table does not have.
                util::append(
                    &tbody,
                    &c::tr(vec![c::td_text(f.window_type), c::td_text(&said)]),
                );
            }
            util::append(card, &t);

            // One button per DEMONSTRATED convention. `opens` is where that
            // rule lives — natively tested, unlike anything in this file.
            for to_open in
                crate::views::registry_browser::output::opens(found, resolved, local_peer)
            {
                // **TWO listeners on one click, and the order is load-bearing.**
                // The window event runs the window's own handler, which
                // registers the origin the signed binding carried and warms that
                // publisher's manifests into MY store; the action then spawns
                // the viewer. Both land in one `actions` queue and drain in
                // registration order, so the origin is registered before the
                // spawned window's factory reads the origin roster to decide
                // what to subscribe — the other order opens a window that cannot
                // see the peer it was opened for.
                //
                // The spawned window is bound to **my** peer, not the
                // publisher's: `peer_id` on a window is the store it reads, and
                // the publisher's cached content lives in mine. `opens` carries
                // that decision and the reasoning; it is where this was wrong.
                //
                // This composition is why the button was broken for its whole
                // life: the window handler alone can only mark itself dirty (it
                // has no way to emit an `Action`), so a control that must open
                // *another* window needs the DOM half.
                let open = c::button(
                    ctx,
                    &crate::i18n::t("registry.open_in", &[("viewer", to_open.window_type)]),
                    c::ButtonKind::Secondary,
                    "registry_open",
                );
                ctx.on_action(
                    &open,
                    "click",
                    crate::action::Action::SpawnWindow {
                        type_name: to_open.window_type,
                        peer_id: Some(to_open.bind_peer),
                        target: Some(to_open.target),
                    },
                );
                util::append(card, &open);
            }
            util::append(
                card,
                &c::pre_notice(&crate::i18n::t("registry.open_caveat", &[])),
            );
        }
    }
}

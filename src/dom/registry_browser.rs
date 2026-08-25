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
    NameListing, Phase, PinOrigin, RegistryBrowserOutput, ResolvedName,
};
use crate::window::WindowId;

const NAME_FIELD: &str = "registry_name";

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

    render_pin(&root, output);
    render_names(&root, output, ctx);
    render_resolve(&root, output, ctx, window_id);

    container.set_inner_html("");
    util::append(container, &root);
}

/// Which registry is in force, and **where it came from**.
///
/// The source line is not decoration [AP25]: a deployment-seeded pin means names
/// resolve through a registry the user never typed. That is the feature, and it
/// is exactly the thing that must not be silent.
fn render_pin(root: &Element, output: &RegistryBrowserOutput) {
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
    util::append(
        &card,
        &c::pre_notice(&crate::i18n::t(
            "registry.sessions",
            &[("n", &output.sessions.to_string())],
        )),
    );
    util::append(root, &card);
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
        Phase::Done(target) => render_evidence(&card, target, ctx),
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
fn render_evidence(card: &Element, target: &ResolvedName, ctx: &DomCtx) {
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

    // The wire to the Site Browser — and the caveat is rendered beside it, not
    // buried in a doc comment. The origin came from a registry-SIGNED binding
    // (better than the deployment list); the pages it then fetches are still
    // origin-trusted, and the Site Browser labels them "not verified".
    if target.origin.is_some() {
        let open = c::button(
            ctx,
            &crate::i18n::t("registry.open_site", &[]),
            c::ButtonKind::Secondary,
            "registry_open",
        );
        util::append(card, &open);
        util::append(
            card,
            &c::pre_notice(&crate::i18n::t("registry.open_caveat", &[])),
        );
    }
}

//! Settings window DOM renderer — pure consumer of
//! [`SettingsOutput`](crate::views::settings::output::SettingsOutput).
//!
//! Every group is a bounded card (S2) and every control is a shared atom
//! (S8): `components::select` (wrapped in `field`), `components::checkbox`,
//! `components::radio`. The `name` / `data-kind` attributes are stable DOM
//! hooks the e2e drives — keep them when touching a control.

use crate::dom::components;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::settings::output::SettingsOutput;

use web_sys::Element;

pub fn render(container: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element("div");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Settings");
    util::append(&wrapper, &h2);

    render_appearance(&wrapper, output, ctx);
    render_windows(&wrapper, output, ctx);
    render_site_surface(&wrapper, output, ctx);
    render_rendering(&wrapper, output, ctx);
    render_network(&wrapper, output, ctx);

    let info = util::create_element("p");
    info.set_attribute("style", "color:var(--text-faint, #666);margin-top:12px;font-size:11px").ok();
    util::set_text(&info, &format!("State: {}", output.state_path));
    util::append(&wrapper, &info);

    util::append(container, &wrapper);
}

fn render_appearance(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    // i18n P1 demonstrators: these labels resolve through `t()` (the string
    // seam). Switch the Language dropdown to the `en-XA` pseudo-locale and they
    // visibly bracket/accent — the end-to-end proof of catalog + pseudolocale +
    // the force-rebuild path. The rest of the app is NOT migrated yet (P4).
    let card = components::card(&crate::i18n::t("settings.appearance", &[]));

    // Theme dropdown (registry-driven — one <option> per registered theme).
    let theme_options: Vec<(&str, &str)> =
        output.themes.iter().map(|o| (o.value, o.label)).collect();
    let theme_selected = output
        .themes
        .iter()
        .find(|o| o.selected)
        .map(|o| o.value)
        .unwrap_or("");
    let theme_select = components::select(ctx, &theme_options, theme_selected, "set_theme");
    theme_select
        .set_attribute("name", &format!("theme-{}", output.window_id))
        .ok();
    util::append(
        &card,
        &components::field(&crate::i18n::t("settings.theme", &[]), "", &theme_select),
    );

    // Language dropdown (registry-driven — one <option> per locale in the
    // roster). Drives `lang`/`dir`; the pseudo-locale (RTL) surfaces layout
    // bugs before real translations exist.
    let lang_options: Vec<(&str, &str)> =
        output.languages.iter().map(|o| (o.value, o.label)).collect();
    let lang_selected = output
        .languages
        .iter()
        .find(|o| o.selected)
        .map(|o| o.value)
        .unwrap_or("");
    let lang_select = components::select(ctx, &lang_options, lang_selected, "set_language");
    lang_select
        .set_attribute("name", &format!("language-{}", output.window_id))
        .ok();
    util::append(
        &card,
        &components::field(
            &crate::i18n::t("settings.language", &[]),
            &crate::i18n::t("settings.language.hint", &[]),
            &lang_select,
        ),
    );

    // Site appearance dropdown — how the Content Site overlay is colored,
    // independent of the chrome theme above (default: the site's own theme).
    let site_options: Vec<(&str, &str)> = output
        .site_appearance
        .iter()
        .map(|o| (o.value.as_str(), o.label.as_str()))
        .collect();
    let site_selected = output
        .site_appearance
        .iter()
        .find(|o| o.selected)
        .map(|o| o.value.as_str())
        .unwrap_or("");
    let site_select =
        components::select(ctx, &site_options, site_selected, "set_site_appearance");
    site_select
        .set_attribute("name", &format!("site-appearance-{}", output.window_id))
        .ok();
    util::append(
        &card,
        &components::field(
            "Site appearance",
            "Controls the in-app site overlay's colors.",
            &site_select,
        ),
    );

    util::append(parent, &card);
}

/// "Windows" — window-manager behavior toggles.
fn render_windows(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let card = components::card(&crate::i18n::t("settings.windows", &[]));
    util::append(
        &card,
        &components::checkbox(
            ctx,
            "singleton_windows",
            output.singleton_windows,
            "toggle_singleton_windows",
            " Single-instance windows: focus an open window instead of opening a duplicate",
        ),
    );
    util::append(parent, &card);
}

/// "Site & Surface" — the UI onto the session config spine (§5). The **startup
/// surface** directly: a (peer, kind, target) triple. One declarative row —
/// pick the peer, the kind (Chrome / Site / Window), and the target; it's
/// stored and boot honors it. (This replaced the old opaque profile-preset
/// selector — the surface IS the setting.) No entity-editing (reframe §7.4).
fn render_site_surface(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let s = &output.session;
    let card = components::card(&crate::i18n::t("settings.site_surface", &[]));

    // -- Startup surface: kind radios --
    let kind_label = util::create_element("span");
    kind_label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&kind_label, "Boot into");
    util::append(&card, &kind_label);
    let kind_row = util::create_element("div");
    kind_row.set_attribute("style", "display:flex;gap:12px;margin:2px 0 8px").ok();
    for (value, text) in [("chrome", "Chrome"), ("site", "Site"), ("window", "Window")] {
        let (row, radio) = components::radio(
            ctx,
            &format!("boot_kind-{}", output.window_id),
            value,
            s.boot_kind == value,
            "set_boot_kind",
            &format!(" {}", text),
        );
        // Stable hook so e2e can target a specific kind regardless of window id.
        radio.set_attribute("data-kind", value).ok();
        util::append(&kind_row, &row);
    }
    util::append(&card, &kind_row);

    // Clarify this is a STARTUP setting — it changes where the next launch
    // lands, not the current view (so enabling "Site" here doesn't abruptly
    // jump you into the overlay mid-edit). Use the status-bar toggle to enter
    // Site Mode now.
    let kind_hint = util::create_element("div");
    kind_hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &kind_hint,
        "Applies at next launch — use the status-bar toggle to enter Site Mode now.",
    );
    util::append(&card, &kind_hint);

    // -- Peer dropdown (the target peer; disabled for Chrome) --
    let peer_options: Vec<(&str, &str)> =
        s.peers.iter().map(|p| (p.id.as_str(), p.label.as_str())).collect();
    let peer_selected = s
        .peers
        .iter()
        .find(|p| p.selected)
        .map(|p| p.id.as_str())
        .unwrap_or("");
    let peer_select = components::select(ctx, &peer_options, peer_selected, "set_boot_peer");
    peer_select.set_attribute("name", "boot_peer").ok();
    if s.target_disabled {
        peer_select.set_attribute("disabled", "").ok();
    }
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.peer", &[]), "", &peer_select),
    );

    // -- Target dropdown (site id or window type; disabled for Chrome) --
    let target_options: Vec<(&str, &str)> = s
        .targets
        .iter()
        .map(|t| (t.value.as_str(), t.label.as_str()))
        .collect();
    let target_selected = s
        .targets
        .iter()
        .find(|t| t.selected)
        .map(|t| t.value.as_str())
        .unwrap_or("");
    let target_select =
        components::select(ctx, &target_options, target_selected, "set_boot_target");
    target_select.set_attribute("name", "boot_target").ok();
    if s.target_disabled {
        target_select.set_attribute("disabled", "").ok();
    }
    if s.targets.is_empty() && !s.target_disabled {
        // Empty target list → nothing to arm (e.g. no sites on the peer).
        let opt = util::create_element("option");
        opt.set_attribute("disabled", "").ok();
        util::set_text(&opt, "(none available)");
        util::append(&target_select, &opt);
    }
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.target", &[]), "", &target_select),
    );

    // Show the chrome ↔ site toggle in the status bar.
    util::append(
        &card,
        &components::checkbox(
            ctx,
            "show_toggle",
            s.show_toggle,
            "toggle_show_toggle",
            " Show the site toggle in the status bar",
        ),
    );

    // Fast-paint checkbox intentionally NOT rendered: the feature is a held
    // seam — gated off (`boot_fast_paint::DISABLED_FOR_CONSOLIDATION`) pending
    // sole ownership of #site-layer by the live overlay, so the toggle would
    // have no observable effect today (D13: don't show a lever that does
    // nothing). The wiring (config field, model toggle, boot reader) is
    // preserved so it can be re-surfaced when the seam reopens. `s.fast_paint`
    // is still read by tests.

    // Lockdown is surfaced read-only so it's visible — it's set by the
    // per-domain deployment config (`site_mode.locked`), not a control here. A
    // user-facing locked toggle waits on the locked-surface SAFETY work (a
    // deliberate, confirmed action with documented recovery), so a tester can't
    // strand themselves with a quiet checkbox.
    if s.locked {
        let note = util::create_element("p");
        note.set_attribute("style", theme::HINT).ok();
        util::set_text(&note, "Lockdown is active (set by this deployment's config).");
        util::append(&card, &note);
    }

    util::append(parent, &card);
}

fn render_rendering(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let card = components::card(&crate::i18n::t("settings.rendering", &[]));
    util::append(
        &card,
        &components::checkbox(
            ctx,
            "show_inspector",
            output.show_inspector,
            "toggle_inspector",
            " Show inspector panel",
        ),
    );
    util::append(parent, &card);
}

fn render_network(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let card = components::card(&crate::i18n::t("settings.network", &[]));
    util::append(
        &card,
        &components::checkbox(
            ctx,
            "auto_connect",
            output.auto_connect,
            "toggle_autoconnect",
            " Auto-connect to known peers on startup",
        ),
    );
    util::append(parent, &card);
}

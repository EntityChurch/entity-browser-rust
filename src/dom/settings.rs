//! Settings window DOM renderer — pure consumer of
//! [`SettingsOutput`](crate::views::settings::output::SettingsOutput).

use crate::dom::util::{self, DomCtx};
use crate::dom::theme;
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
    let appearance = util::create_element("div");
    appearance.set_attribute("style", theme::SECTION_GROUP).ok();
    let h3 = util::create_element("h3");
    h3.set_attribute("style", "margin:0 0 4px;font-size:14px").ok();
    util::set_text(&h3, "Appearance");
    util::append(&appearance, &h3);

    // Theme dropdown (registry-driven — one <option> per registered theme).
    let theme_label = util::create_element("label");
    theme_label
        .set_attribute("style", "display:block;margin-bottom:8px")
        .ok();
    let theme_span = util::create_element("span");
    theme_span.set_attribute("style", theme::LABEL).ok();
    util::set_text(&theme_span, "Theme");
    util::append(&theme_label, &theme_span);
    let theme_select = util::create_element("select");
    theme_select.set_attribute("style", theme::INPUT).ok();
    theme_select
        .set_attribute("name", &format!("theme-{}", output.window_id))
        .ok();
    for option in &output.themes {
        let opt = util::create_element("option");
        opt.set_attribute("value", option.value).ok();
        if option.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, option.label);
        util::append(&theme_select, &opt);
    }
    ctx.on_select_change(&theme_select, "set_theme");
    util::append(&theme_label, &theme_select);
    util::append(&appearance, &theme_label);

    // Site appearance dropdown — how the Content Site overlay is colored,
    // independent of the chrome theme above (default: the site's own theme).
    let site_label = util::create_element("label");
    site_label
        .set_attribute("style", "display:block;margin-bottom:4px")
        .ok();
    let site_span = util::create_element("span");
    site_span.set_attribute("style", theme::LABEL).ok();
    util::set_text(&site_span, "Site appearance");
    util::append(&site_label, &site_span);
    let site_select = util::create_element("select");
    site_select.set_attribute("style", theme::INPUT).ok();
    site_select
        .set_attribute("name", &format!("site-appearance-{}", output.window_id))
        .ok();
    for option in &output.site_appearance {
        let opt = util::create_element("option");
        opt.set_attribute("value", &option.value).ok();
        if option.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &option.label);
        util::append(&site_select, &opt);
    }
    ctx.on_select_change(&site_select, "set_site_appearance");
    util::append(&site_label, &site_select);
    util::append(&appearance, &site_label);

    // One-line hint: the override exists because sites carry no theme of their
    // own yet — "Match system theme" makes them read cleanly in light/dark.
    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, "Controls the in-app site overlay's colors.");
    util::append(&appearance, &hint);

    util::append(parent, &appearance);
}

/// "Windows" — window-manager behavior toggles.
fn render_windows(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let group = util::create_element("div");
    group.set_attribute("style", theme::SECTION_GROUP).ok();
    let h3 = util::create_element("h3");
    h3.set_attribute("style", "margin:0 0 4px;font-size:14px").ok();
    util::set_text(&h3, "Windows");
    util::append(&group, &h3);

    checkbox_row(
        &group,
        ctx,
        "singleton_windows",
        output.singleton_windows,
        "toggle_singleton_windows",
        " Single-instance windows: focus an open window instead of opening a duplicate",
    );

    util::append(parent, &group);
}

/// "Site & Surface" — the UI onto the session config spine (§5). Profile
/// preset + the **startup surface**: a (peer, kind, target) triple. One
/// declarative row — pick the peer, the kind (Chrome / Site / Window), and the
/// target; it's stored and boot honors it. No entity-editing (reframe §7.4).
fn render_site_surface(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let s = &output.session;
    let group = util::create_element("div");
    group.set_attribute("style", theme::SECTION_GROUP).ok();
    let h3 = util::create_element("h3");
    h3.set_attribute("style", "margin:0 0 4px;font-size:14px").ok();
    util::set_text(&h3, "Site & Surface");
    util::append(&group, &h3);

    // Profile preset selector.
    let profile_label = util::create_element("label");
    profile_label.set_attribute("style", "display:block;margin-bottom:8px").ok();
    let plabel_span = util::create_element("span");
    plabel_span.set_attribute("style", theme::LABEL).ok();
    util::set_text(&plabel_span, "Profile");
    util::append(&profile_label, &plabel_span);
    let select = util::create_element("select");
    select.set_attribute("style", theme::INPUT).ok();
    for p in &s.profiles {
        let opt = util::create_element("option");
        opt.set_attribute("value", p.value).ok();
        if p.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, p.label);
        util::append(&select, &opt);
    }
    ctx.on_select_change(&select, "set_profile");
    util::append(&profile_label, &select);
    util::append(&group, &profile_label);

    // -- Startup surface: kind radios --
    let kind_label = util::create_element("span");
    kind_label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&kind_label, "Boot into");
    util::append(&group, &kind_label);
    let kind_row = util::create_element("div");
    kind_row.set_attribute("style", "display:flex;gap:12px;margin:2px 0 8px").ok();
    for (value, text) in [("chrome", "Chrome"), ("site", "Site"), ("window", "Window")] {
        let label = util::create_element("label");
        label.set_attribute("style", theme::LABEL_CHOICE).ok();
        let radio = util::create_element("input");
        radio.set_attribute("type", "radio").ok();
        radio.set_attribute("name", &format!("boot_kind-{}", output.window_id)).ok();
        // Stable hook so e2e can target a specific kind regardless of window id.
        radio.set_attribute("data-kind", value).ok();
        if s.boot_kind == value {
            radio.set_attribute("checked", "").ok();
        }
        ctx.on_window_event(&radio, "click", "set_boot_kind", value);
        util::append(&label, &radio);
        let span = util::create_element("span");
        util::set_text(&span, &format!(" {}", text));
        util::append(&label, &span);
        util::append(&kind_row, &label);
    }
    util::append(&group, &kind_row);

    // Clarify this is a STARTUP setting — it changes where the next launch
    // lands, not the current view (so enabling "Site" here doesn't abruptly
    // jump you into the overlay mid-edit). Use the status-bar toggle to enter
    // Site Mode now.
    let kind_hint = util::create_element("div");
    kind_hint
        .set_attribute("style", "font-size:11px;color:#7a8294;margin:-4px 0 8px")
        .ok();
    util::set_text(
        &kind_hint,
        "Applies at next launch — use the status-bar toggle to enter Site Mode now.",
    );
    util::append(&group, &kind_hint);

    // -- Peer dropdown (the target peer; disabled for Chrome) --
    let peer_label = util::create_element("label");
    peer_label.set_attribute("style", "display:block;margin-bottom:8px").ok();
    let peer_span = util::create_element("span");
    peer_span.set_attribute("style", theme::LABEL).ok();
    util::set_text(&peer_span, "Peer");
    util::append(&peer_label, &peer_span);
    let peer_select = util::create_element("select");
    peer_select.set_attribute("style", theme::INPUT).ok();
    peer_select.set_attribute("name", "boot_peer").ok();
    if s.target_disabled {
        peer_select.set_attribute("disabled", "").ok();
    }
    for p in &s.peers {
        let opt = util::create_element("option");
        opt.set_attribute("value", &p.id).ok();
        if p.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &p.label);
        util::append(&peer_select, &opt);
    }
    ctx.on_select_change(&peer_select, "set_boot_peer");
    util::append(&peer_label, &peer_select);
    util::append(&group, &peer_label);

    // -- Target dropdown (site id or window type; disabled for Chrome) --
    let target_label = util::create_element("label");
    target_label.set_attribute("style", "display:block;margin-bottom:8px").ok();
    let target_span = util::create_element("span");
    target_span.set_attribute("style", theme::LABEL).ok();
    util::set_text(&target_span, "Target");
    util::append(&target_label, &target_span);
    let target_select = util::create_element("select");
    target_select.set_attribute("style", theme::INPUT).ok();
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
    for t in &s.targets {
        let opt = util::create_element("option");
        opt.set_attribute("value", &t.value).ok();
        if t.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &t.label);
        util::append(&target_select, &opt);
    }
    ctx.on_select_change(&target_select, "set_boot_target");
    util::append(&target_label, &target_select);
    util::append(&group, &target_label);

    // Show the chrome ↔ site toggle in the status bar.
    checkbox_row(&group, ctx, "show_toggle", s.show_toggle, "toggle_show_toggle", " Show the site toggle in the status bar");

    // Fast-paint checkbox intentionally NOT rendered: the feature is a held
    // seam — gated off (`boot_fast_paint::DISABLED_FOR_CONSOLIDATION`) pending
    // sole ownership of #site-layer by the live overlay, so the toggle would
    // have no observable effect today (D13: don't show a lever that does
    // nothing). The wiring (config field, model toggle, boot reader) is
    // preserved so it can be re-surfaced when the seam reopens. `s.fast_paint`
    // is still read by tests.

    // Lockdown is a held seam — surface it read-only so it's visible, but no
    // control flips it yet (§4-C).
    if s.locked {
        let note = util::create_element("p");
        note.set_attribute("style", "color:var(--text-dim, #888);margin:4px 0 0;font-size:11px").ok();
        util::set_text(&note, "Lockdown is active (set by profile).");
        util::append(&group, &note);
    }

    util::append(parent, &group);
}

/// A labeled checkbox row that fires a `WindowEvent` on change. `name` is a
/// stable DOM hook so e2e (and any other consumer) can target a specific
/// checkbox rather than "all checkboxes."
fn checkbox_row(parent: &Element, ctx: &DomCtx, name: &str, checked: bool, event: &str, label_text: &str) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL_CHOICE).ok();
    let cb = util::create_element("input");
    cb.set_attribute("type", "checkbox").ok();
    cb.set_attribute("name", name).ok();
    if checked {
        cb.set_attribute("checked", "").ok();
    }
    ctx.on_window_event(&cb, "change", event, "");
    util::append(&label, &cb);
    let span = util::create_element("span");
    util::set_text(&span, label_text);
    util::append(&label, &span);
    util::append(parent, &label);
}

fn render_rendering(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let rendering = util::create_element("div");
    rendering.set_attribute("style", theme::SECTION_GROUP).ok();
    let h3 = util::create_element("h3");
    h3.set_attribute("style", "margin:0 0 4px;font-size:14px").ok();
    util::set_text(&h3, "Rendering");
    util::append(&rendering, &h3);

    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL_CHOICE).ok();
    let cb = util::create_element("input");
    cb.set_attribute("type", "checkbox").ok();
    cb.set_attribute("name", "show_inspector").ok();
    if output.show_inspector {
        cb.set_attribute("checked", "").ok();
    }
    ctx.on_window_event(&cb, "change", "toggle_inspector", "");
    util::append(&label, &cb);
    let span = util::create_element("span");
    util::set_text(&span, " Show inspector panel");
    util::append(&label, &span);
    util::append(&rendering, &label);

    util::append(parent, &rendering);
}

fn render_network(parent: &Element, output: &SettingsOutput, ctx: &DomCtx) {
    let network = util::create_element("div");
    network.set_attribute("style", theme::SECTION_GROUP).ok();
    let h3 = util::create_element("h3");
    h3.set_attribute("style", "margin:0 0 4px;font-size:14px").ok();
    util::set_text(&h3, "Network");
    util::append(&network, &h3);

    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL_CHOICE).ok();
    let cb = util::create_element("input");
    cb.set_attribute("type", "checkbox").ok();
    cb.set_attribute("name", "auto_connect").ok();
    if output.auto_connect {
        cb.set_attribute("checked", "").ok();
    }
    ctx.on_window_event(&cb, "change", "toggle_autoconnect", "");
    util::append(&label, &cb);
    let span = util::create_element("span");
    util::set_text(&span, " Auto-connect to known peers on startup");
    util::append(&label, &span);
    util::append(&network, &label);

    util::append(parent, &network);
}

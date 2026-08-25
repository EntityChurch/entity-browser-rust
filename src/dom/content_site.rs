//! DOM renderer for the Content Site window.
//!
//! Pure construction: takes a [`SiteRenderOutput`] and builds DOM into
//! the given container. Renders the simple site layout — a top nav bar,
//! and a single centered content pane (the rendered markdown). Does not
//! touch the tree, model, or peers.
//!
//! Two link concerns live here (they need `ctx`):
//! - **Nav menu** items dispatch [`Action::SiteNavigate`] on click.
//! - **In-page `<a>` links** are rewritten after mount: in-system links
//!   `preventDefault` + dispatch `SiteNavigate`; external links are
//!   left as real `target="_blank"` anchors (they leave the system).
//!
//! The container is host-agnostic — a window section in P1, the
//! full-screen `#site-layer` overlay in P2. This renderer is identical
//! either way.

use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::action::Action;
use crate::content_site::{classify_link, paths, LinkTarget, Location, PageRender};
use crate::dom::{util, DomCtx};
use crate::views::content_site::output::{NavLink, SectionLink, SiteRenderOutput};
use crate::window::WindowId;

/// Which surface hosts this render — decides the nav action a link
/// click dispatches. The renderer is otherwise identical for both.
#[derive(Debug, Clone, Copy)]
pub enum SiteNavHost {
    /// A Content Site **window** section — nav routes to that window.
    Window(WindowId),
    /// The full-screen Site Mode **overlay** (app-level surface).
    ///
    /// `can_exit` gates the "Exit Site ▲" control: it is the overlay-side
    /// chrome↔site toggle and shows iff the deployment exposes that toggle
    /// (`site_mode.show_toggle && enabled`). In a **locked** deployment
    /// (strict-site: `show_toggle=false`, `locked=true`) it is `false`, so the
    /// overlay renders no exit — closing BUG-1, the "Exit Site strands you in
    /// chrome with no way back" footgun. The action is *also* guarded
    /// (`ToggleSiteMode` no-ops when `locked`) as defense in depth.
    Overlay { can_exit: bool },
}

impl SiteNavHost {
    /// The navigation action a link/menu click should dispatch for this
    /// host, given the raw link `target`.
    fn nav_action(&self, target: String) -> Action {
        match self {
            SiteNavHost::Window(window_id) => Action::SiteNavigate {
                window_id: *window_id,
                target,
            },
            SiteNavHost::Overlay { .. } => Action::SiteOverlayNavigate { target },
        }
    }

    /// The "go back" action for this host.
    fn back_action(&self) -> Action {
        match self {
            SiteNavHost::Window(window_id) => Action::SiteBack { window_id: *window_id },
            SiteNavHost::Overlay { .. } => Action::SiteOverlayBack,
        }
    }
}

/// Resolves an embed `ref` (`assets/figures/x.png`) to its `(media_type,
/// bytes)`, or `None` if it isn't a resolvable site-local asset. Built by the
/// caller over the live store (it has `peers` + the bound peer id); used
/// post-mount by [`rewrite_images`] to turn `<img src="assets/…">` into an
/// inline `data:` URL. Borrowing (not `'static`) — it runs during `render`.
pub type AssetResolver<'a> = dyn Fn(&str) -> Option<(String, Vec<u8>)> + 'a;

/// Build the asset resolver for one render pass over the live store. Reads use
/// the **selector/path split** the cache uses ([`resolve_from_my_store`]): the
/// selector is always the **bound peer** (MY store — both owned assets and
/// cached-foreign write-throughs live there), while the asset path's
/// peer-segment is the page's owning peer (`output.peer` for a foreign site,
/// the bound peer for an owned one). Resolution is L0/sync (Direct reads the
/// store, Worker the cache mirror — fed by the surface's `sites/`
/// subscription). Borrows `peers`; the closure is used immediately by
/// [`render`], never stored.
///
/// [`resolve_from_my_store`]: crate::content_site::resolver
pub fn make_asset_resolver<'a>(
    peers: &'a crate::peers::Peers,
    bound_peer_id: &str,
    output: &SiteRenderOutput,
) -> impl Fn(&str) -> Option<(String, Vec<u8>)> + 'a {
    use crate::content_site::format::{SiteAsset, SITE_ASSET_TYPE};
    use crate::content_site::paths;
    let selector = bound_peer_id.to_string();
    let path_peer = output.peer.clone().unwrap_or_else(|| bound_peer_id.to_string());
    let site_id = output.site_id.clone();
    move |reference: &str| {
        let name = paths::asset_name_from_ref(reference)?;
        let entity = peers.get_entity(&selector, &paths::asset_path(&path_peer, &site_id, &name))?;
        if entity.entity_type != SITE_ASSET_TYPE {
            return None;
        }
        let asset = SiteAsset::from_entity(&entity);
        Some((asset.media_type, asset.bytes))
    }
}

/// Responsive layout CSS for the site view, injected as a `<style>` by
/// [`render`]. The shared renderer serves both hosts: the **overlay** mounts
/// into the light-DOM `#site-layer` (which never gets `dom::style::DOM_STYLES`),
/// so the renderer must carry its own rules; a `<style>` element's rules are
/// scoped to the containing root, so this also reaches the window's directory
/// rail in the shadow DOM. Class names are `cs-*` to avoid collisions.
///
/// Desktop: the body is a row with a fixed-width section sidebar (the
/// current look). Narrow screens (≤768px, e.g. mobile in overlay mode):
/// the body **stacks vertically**, the sidebar collapses behind a "Contents"
/// toggle (default closed, so the page content is visible first instead of a
/// side column eating half the screen), and the toggle reveals it inline.
const RESPONSIVE_CSS: &str = "\
.cs-nav-desktop{display:flex;align-items:center;gap:14px;flex:1;min-width:0;}\
.cs-nav-burger{display:none;}\
.cs-nav-menu{display:none;}\
.cs-body{display:flex;flex:1;min-height:0;overflow:hidden;}\
.cs-main{flex:1;min-width:0;overflow:auto;}\
.cs-main,.cs-main *{box-sizing:border-box;}\
.cs-main-doc{display:flex;flex-direction:column;}\
.cs-pane-doc{flex:1;min-height:0;display:flex;padding:0;}\
.cs-docframe{flex:1;width:100%;display:block;border:0;min-height:70vh;}\
.cs-sidebar{flex-shrink:0;width:210px;overflow:auto;padding:18px 12px;\
border-inline-end:1px solid var(--site-border, #20202e);\
background:var(--site-sidebar-bg, #13131c);display:flex;\
flex-direction:column;gap:2px;}\
.cs-sidebar-toggle{display:none;}\
.cs-sidebar-list{display:flex;flex-direction:column;gap:2px;}\
@media (max-width:768px){\
.cs-nav-desktop{display:none;}\
.cs-nav-burger{display:flex;align-items:center;justify-content:center;\
margin-inline-start:auto;flex-shrink:0;background:var(--site-control-bg, #22223a);\
color:var(--site-control-text, #cfe3ff);\
border:1px solid var(--site-control-border, #3a3a52);border-radius:6px;\
padding:6px 12px;font-size:17px;line-height:1;cursor:pointer;}\
.cs-nav-menu.cs-open{display:flex;}\
.cs-nav-menu{position:absolute;top:calc(100% + 4px);left:8px;right:8px;\
flex-direction:column;gap:2px;background:var(--site-panel-bg, #1b1b28);\
border:1px solid var(--site-panel-border, #2f2f46);\
border-radius:8px;padding:8px;z-index:70;box-shadow:0 12px 32px rgba(0,0,0,0.55);\
max-height:75vh;overflow:auto;}\
.cs-body{flex-direction:column;overflow:auto;}\
.cs-main{overflow:visible;}\
.cs-sidebar{width:auto;overflow:visible;padding:8px 12px;border-inline-end:none;\
border-bottom:1px solid var(--site-border, #20202e);gap:0;}\
.cs-sidebar-toggle{display:flex;align-items:center;justify-content:space-between;\
width:100%;background:var(--site-toggle-bg, #1a1a26);\
color:var(--site-text-strong, #c3c9d6);\
border:1px solid var(--site-border-2, #2a2a3e);\
border-radius:6px;padding:9px 12px;font-family:inherit;font-size:13px;\
cursor:pointer;}\
.cs-sidebar-list{display:none;padding-top:8px;max-height:50vh;overflow:auto;}\
.cs-sidebar.cs-open .cs-sidebar-list{display:flex;}\
}\
.cs-window-row{display:flex;height:100%;overflow:hidden;}\
.cs-rail{flex-shrink:0;width:188px;overflow:auto;padding:14px 10px;\
border-inline-end:1px solid var(--site-border, #20202e);\
background:var(--site-rail-bg, #0d0d14);display:flex;\
flex-direction:column;gap:3px;}\
.cs-rail-toggle{display:none;}\
.cs-rail-list{display:flex;flex-direction:column;gap:3px;}\
@media (max-width:768px){\
.cs-window-row{flex-direction:column;overflow:auto;}\
.cs-rail{width:auto;overflow:visible;padding:8px 10px;border-inline-end:none;\
border-bottom:1px solid var(--site-border, #20202e);gap:0;}\
.cs-rail-toggle{display:flex;align-items:center;justify-content:space-between;\
width:100%;background:var(--site-toggle-bg-rail, #14141f);\
color:var(--site-text-strong, #c3c9d6);\
border:1px solid var(--site-border-2, #2a2a3e);\
border-radius:6px;padding:9px 12px;font-family:inherit;font-size:13px;\
cursor:pointer;}\
.cs-rail-list{display:none;padding-top:8px;max-height:45vh;overflow:auto;}\
.cs-rail.cs-open .cs-rail-list{display:flex;}\
}";

/// Build the Content Site content into `container` for the given `host`.
pub fn render(
    container: &Element,
    output: &SiteRenderOutput,
    ctx: &DomCtx,
    host: SiteNavHost,
    resolve_asset: &AssetResolver,
) {
    util::clear_children(container);

    // Responsive layout rules (root-scoped; see `RESPONSIVE_CSS`) + the ONE
    // content-document stylesheet (`content_site::doc_css`, S-T1) in its
    // live token-with-fallback form — the same rule table the static
    // exporter freezes, so the overlay and a published page can't drift.
    // Built once (render runs on every snapshot rebuild; the sheet is static).
    static OVERLAY_CSS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let css = OVERLAY_CSS.get_or_init(|| {
        format!(
            "{}{}",
            RESPONSIVE_CSS,
            crate::content_site::doc_css::doc_css(
                ".cs-doc",
                crate::content_site::doc_css::PaletteMode::Live
            )
        )
    });
    let style = util::create_element("style");
    util::set_text(&style, css);
    util::append(container, &style);

    let wrapper = util::create_element("div");
    // The manifest-declared site theme (S-T2) rides in as container-scoped
    // custom properties on the site's OWN wrapper — the whole subtree
    // (nav/sidebar/doc rules resolve `var(--site-*)` per element) takes the
    // palette, while any second site surface and the `:root` layer stay
    // untouched. The output field is already mode-gated and registry-
    // validated (see `SiteRenderOutput::site_theme_css`); dropped with the
    // wrapper on every rebuild, so site-switch/exit cleanup is structural.
    let mut wrapper_style = String::from(
        "display:flex;flex-direction:column;height:100%;overflow:hidden;\
         background:var(--site-bg, #101018);\
         font-family:system-ui,-apple-system,sans-serif;",
    );
    if let Some(vars) = &output.site_theme_css {
        wrapper_style.push_str(vars);
    }
    util::set_attr(&wrapper, "style", &wrapper_style);

    render_nav_bar(&wrapper, output, ctx, host);

    // A sidebar appears only when the site has tree structure (the
    // model's `.list`-derived `sidebar`). A flat site keeps the simple
    // single-pane layout. The body row scrolls; the nav bar stays pinned.
    // On mobile `.cs-body` stacks the (collapsible) sidebar above the content.
    let has_sidebar = !output.sidebar.is_empty();
    let body_row = util::create_element_with_class("div", "cs-body");
    if has_sidebar {
        render_sidebar(&body_row, output, ctx, host);
    }

    // The main column (breadcrumbs + content pane) scrolls independently.
    let main = util::create_element_with_class("div", "cs-main");
    render_breadcrumbs(&main, output, ctx, host);
    render_content(&main, output, ctx, host, resolve_asset);
    util::append(&body_row, &main);

    util::append(&wrapper, &body_row);
    util::append(container, &wrapper);
}

/// How many nav items render inline before the rest collapse into the
/// "More ▾" overflow dropdown. A flat 16-item nav (billslab-papers) shows
/// the first few inline + a dropdown for the tail, instead of a long
/// scrolling strip. Kept conservative so the inline items reliably fit at
/// common widths (the strip clips rather than scrolls — no ugly scrollbar).
const NAV_INLINE_MAX: usize = 4;

/// The top nav bar. Always visible: a left cluster (back + clickable Home
/// title). The rest is **two layouts the responsive CSS swaps between**:
///
/// - **Desktop** (`.cs-nav-desktop`): up to [`NAV_INLINE_MAX`] inline nav items,
///   surplus under a "More ▾" dropdown, then a right cluster (Share + overlay
///   Exit) pinned via `margin-inline-start:auto`.
/// - **Mobile** (≤768px): the desktop region is hidden; a **hamburger ☰**
///   opens a single vertical dropdown (`.cs-nav-menu`) with *every* nav link +
///   Share + Exit. The panel is viewport-anchored (`left:8px;right:8px`), so
///   nothing clips off-screen — fixing the mobile "More panel off-screen / Share
///   off the edge" bug. No inline/overflow split on mobile.
fn render_nav_bar(wrapper: &Element, output: &SiteRenderOutput, ctx: &DomCtx, host: SiteNavHost) {
    let can_exit = matches!(host, SiteNavHost::Overlay { can_exit: true });

    let bar = util::create_element("div");
    // `position:relative` anchors the mobile dropdown panel to the bar.
    util::set_attr(
        &bar,
        "style",
        "position:relative;display:flex;align-items:center;gap:14px;\
         padding:10px 18px;border-bottom:1px solid var(--site-border-2, #2a2a3e);\
         background:var(--site-nav-bg, #15151f);flex-shrink:0;",
    );

    // -- Left cluster: back + Home title (always visible). Allowed to shrink
    //    (the title ellipsizes) so the mobile hamburger is never pushed off. --
    let left = util::create_element("div");
    util::set_attr(
        &left,
        "style",
        "display:flex;align-items:center;gap:12px;min-width:0;",
    );

    // Back affordance — only when there's somewhere to go back to.
    if output.can_go_back {
        let back = util::create_element("button");
        util::set_text(&back, "\u{2190}");
        util::set_attr(&back, "title", &crate::i18n::t("tooltip.back", &[]));
        util::set_attr(
            &back,
            "style",
            "background:var(--site-control-bg, #22223a);\
             color:var(--site-control-text, #cfe3ff);\
             border:1px solid var(--site-control-border, #3a3a52);\
             border-radius:4px;padding:2px 9px;font-size:14px;cursor:pointer;\
             font-family:inherit;line-height:1.2;flex-shrink:0;",
        );
        ctx.on_action(&back, "click", host.back_action());
        util::append(&left, &back);
    }

    // The site title doubles as the Home button. In a **window** it navigates
    // to the current site's root (`"/"` → the manifest home page) — per-site, as
    // you browse many. In the **overlay** it navigates to the deployment's
    // configured home site (`output.home_target`, a `site:`/`entity://` link)
    // so it ALWAYS resets to the real site — even when the current location is
    // unresolvable ("No site manifest…"), where `/` would just reload the error
    // and strand a locked deployment. Falls back to `/` when no home is carried.
    // A "⌂" glyph signals the affordance; it ellipsizes rather than overflow.
    let home = util::create_element("a");
    util::set_text(&home, &format!("\u{2302}  {}", output.site_title));
    util::set_attr(&home, "href", "#");
    util::set_attr(&home, "title", &crate::i18n::t("tooltip.site_home", &[]));
    util::set_attr(
        &home,
        "style",
        "color:var(--site-control-text, #cfe3ff);font-size:15px;font-weight:600;\
         text-decoration:none;white-space:nowrap;cursor:pointer;overflow:hidden;\
         text-overflow:ellipsis;",
    );
    let home_target = match host {
        SiteNavHost::Overlay { .. } if !output.home_target.is_empty() => output.home_target.clone(),
        _ => "/".to_string(),
    };
    wire_nav(ctx, &home, home_target, host);
    util::append(&left, &home);
    util::append(&bar, &left);

    // -- Desktop region: inline items + "More ▾" + Share/Exit right cluster --
    let desktop = util::create_element_with_class("div", "cs-nav-desktop");
    render_nav_items(&desktop, &output.nav, ctx, host);
    let right = util::create_element("div");
    util::set_attr(
        &right,
        "style",
        "display:flex;align-items:center;gap:10px;flex-shrink:0;margin-inline-start:auto;",
    );
    render_share_button(&right, output, ctx, false);
    if can_exit {
        exit_button(&right, ctx, false);
    }
    util::append(&desktop, &right);
    util::append(&bar, &desktop);

    // -- Mobile: hamburger + a single vertical dropdown of everything --
    let burger = util::create_element_with_class("button", "cs-nav-burger");
    util::set_attr(&burger, "type", "button");
    util::set_attr(&burger, "title", &crate::i18n::t("tooltip.menu", &[]));
    util::set_text(&burger, "\u{2630}"); // ☰
    let menu = util::create_element_with_class("div", "cs-nav-menu");
    for link in &output.nav {
        util::append(&menu, &nav_anchor(ctx, link, host, true));
    }
    // Divider, then the chrome actions, so links and actions read as groups.
    let divider = util::create_element("div");
    util::set_attr(
        &divider,
        "style",
        "height:1px;background:var(--site-panel-border, #2f2f46);margin:6px 2px;",
    );
    util::append(&menu, &divider);
    render_share_button(&menu, output, ctx, true);
    if can_exit {
        exit_button(&menu, ctx, true);
    }
    // The hamburger toggles the menu open/closed (DOM-held `cs-open`, like the
    // sidebar toggle — survives idle frames, resets on the next rebuild).
    ctx.listen(&burger, "click", {
        let menu = menu.clone();
        move |evt: web_sys::Event| {
            evt.stop_propagation();
            let _ = menu.class_list().toggle("cs-open");
        }
    });
    util::append(&bar, &burger);
    util::append(&bar, &menu);

    util::append(wrapper, &bar);
}

/// Build the nav region: up to [`NAV_INLINE_MAX`] items inline, the rest under a
/// "More ▾" dropdown. The inline strip can still horizontally scroll as a last
/// resort at very narrow widths, but the dropdown keeps the common case tidy
/// (no long scroll strip). The active page is always shown inline.
fn render_nav_items(bar: &Element, nav: &[NavLink], ctx: &DomCtx, host: SiteNavHost) {
    if nav.is_empty() {
        return;
    }

    // The inline strip. `flex:0 1 auto` lets it shrink under pressure without
    // growing to eat the bar; `overflow:hidden` means it clips (never a
    // scrollbar) in the rare case the few inline items don't fit — the bulk of
    // a long nav lives in the "More ▾" dropdown, not a scroll strip. Padding
    // gives the items a little breathing room.
    let strip = util::create_element("nav");
    util::set_attr(
        &strip,
        "style",
        "display:flex;align-items:center;gap:16px;flex:0 1 auto;min-width:0;\
         overflow:hidden;padding:4px 2px;",
    );

    // Partition into inline + overflow, then ensure the active item is inline
    // (swap it in for the last inline slot if it landed in the overflow).
    let mut inline: Vec<&NavLink> = nav.iter().take(NAV_INLINE_MAX).collect();
    let mut overflow: Vec<&NavLink> = nav.iter().skip(NAV_INLINE_MAX).collect();
    if !inline.iter().any(|l| l.active) {
        if let Some(pos) = overflow.iter().position(|l| l.active) {
            let active = overflow.remove(pos);
            if let Some(displaced) = inline.pop() {
                overflow.insert(0, displaced);
            }
            inline.push(active);
        }
    }

    for link in &inline {
        util::append(&strip, &nav_anchor(ctx, link, host, false));
    }
    util::append(bar, &strip);

    if !overflow.is_empty() {
        render_more_dropdown(bar, &overflow, ctx, host);
    }
}

/// One nav menu anchor. `block` styles it as a full-width dropdown row;
/// otherwise it's an inline bar item. Active items are highlighted.
fn nav_anchor(ctx: &DomCtx, link: &NavLink, host: SiteNavHost, block: bool) -> Element {
    let a = util::create_element("a");
    util::set_text(&a, &link.label);
    util::set_attr(&a, "href", "#");
    let color = if link.active {
        "color:var(--site-accent, #9fd0ff);font-weight:bold;"
    } else {
        "color:var(--site-text-muted, #9aa3b2);"
    };
    let layout = if block {
        "display:block;padding:6px 10px;border-radius:4px;"
    } else {
        "flex-shrink:0;white-space:nowrap;"
    };
    util::set_attr(&a, "style", &format!("text-decoration:none;{layout}{color}"));
    wire_nav(ctx, &a, link.target.clone(), host);
    a
}

/// Append the "More ▾" overflow control: a button that toggles a dropdown panel
/// listing the overflow nav items. The panel lives in the bar (NOT inside the
/// scrollable strip — an absolutely-positioned panel there would be clipped by
/// `overflow-x:auto`). Toggle state lives in the DOM (the panel's `display`);
/// it survives idle frames (the overlay's rebuild guard) and resets closed on
/// the next rebuild (i.e. after a navigation). No tree state — it's ephemeral
/// chrome.
fn render_more_dropdown(bar: &Element, overflow: &[&NavLink], ctx: &DomCtx, host: SiteNavHost) {
    let wrap = util::create_element("div");
    util::set_attr(&wrap, "style", "position:relative;flex-shrink:0;");

    let btn = util::create_element("button");
    util::set_attr(&btn, "type", "button");
    util::set_text(
        &btn,
        &crate::i18n::t("contentsite.more", &[("n", &overflow.len().to_string())]),
    );
    util::set_attr(
        &btn,
        "style",
        "background:var(--site-control-bg, #22223a);\
         color:var(--site-control-text, #cfe3ff);\
         border:1px solid var(--site-control-border, #3a3a52);\
         border-radius:4px;padding:4px 12px;font-size:13px;cursor:pointer;\
         font-family:inherit;white-space:nowrap;",
    );
    util::append(&wrap, &btn);

    let panel = util::create_element("div");
    // Base style shared by the open/closed variants; only `display` differs.
    const PANEL_BASE: &str = "position:absolute;top:calc(100% + 6px);inset-inline-start:0;\
         min-width:180px;max-height:60vh;overflow-y:auto;\
         background:var(--site-panel-bg, #1b1b28);\
         border:1px solid var(--site-panel-border, #2f2f46);border-radius:6px;\
         padding:6px;z-index:60;box-shadow:0 8px 24px rgba(0,0,0,0.5);\
         flex-direction:column;gap:2px;";
    util::set_attr(&panel, "style", &format!("{PANEL_BASE}display:none;"));
    for link in overflow {
        util::append(&panel, &nav_anchor(ctx, link, host, true));
    }
    util::append(&wrap, &panel);

    // Toggle on click (open ⇄ closed). `stop_propagation` so the click doesn't
    // bubble to any future document-level close handler.
    let open_style = format!("{PANEL_BASE}display:flex;");
    let closed_style = format!("{PANEL_BASE}display:none;");
    ctx.listen(&btn, "click", {
        let panel = panel.clone();
        move |evt: web_sys::Event| {
            evt.stop_propagation();
            let is_open = panel
                .get_attribute("style")
                .map(|s| s.contains("display:flex"))
                .unwrap_or(false);
            let next = if is_open { &closed_style } else { &open_style };
            panel.set_attribute("style", next).ok();
        }
    });

    util::append(bar, &wrap);
}

/// Append the share control — a single **"🔗 Share link"** that copies the
/// same-origin `?site=` deep link ([`paths::self_deep_link`], the `self`
/// sentinel — the *same* link the static banner emits) re-opening this page in
/// the live entity browser. Always works same-origin.
///
/// **Why no "static link" here (removed):** a static permalink is
/// peer-qualified (`/sites/{peer}/…`), but the live app shows ITS OWN
/// (ephemeral, localStorage) system peer's site, which is NOT statically
/// published anywhere the app knows — `make site`/`site-serve` exports a
/// SEPARATE ephemeral publish peer. So a static link built from the live
/// peer-id 404s (the reported bug). A working live→static link needs the
/// hosting-identity piece: the live peer publishing its OWN tree, or a registry
/// (`content_site::origins`, `peer_id → origin`) telling the app where this
/// peer's site is published. Until then we only offer the live link (the
/// same-origin round-trip that works) and the static banner (static→live).
fn render_share_button(bar: &Element, output: &SiteRenderOutput, ctx: &DomCtx, block: bool) {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    let live_link = paths::self_deep_link(&origin, &output.site_id, &output.current_page);
    share_button(
        bar,
        ctx,
        &crate::i18n::t("contentsite.share_link", &[]),
        &crate::i18n::t("contentsite.share_link_hint", &[]),
        live_link,
        block,
    );
}

/// One copy-to-clipboard share button. `block` = a full-width mobile-menu row;
/// otherwise an inline desktop-chrome button (positioning owned by the caller's
/// right-hand cluster in [`render_nav_bar`]). Flips its label to "Copied ✓" on
/// click.
fn share_button(bar: &Element, ctx: &DomCtx, label: &str, title: &str, link: String, block: bool) {
    let btn = util::create_element("button");
    util::set_text(&btn, label);
    util::set_attr(&btn, "title", title);
    let style = if block {
        "display:block;width:100%;text-align:start;\
         background:var(--site-control-bg, #22223a);\
         color:var(--site-control-text, #cfe3ff);\
         border:1px solid var(--site-control-border, #3a3a52);\
         border-radius:4px;padding:8px 10px;font-size:13px;\
         cursor:pointer;font-family:inherit;"
    } else {
        "background:var(--site-control-bg, #22223a);\
         color:var(--site-control-text, #cfe3ff);\
         border:1px solid var(--site-control-border, #3a3a52);\
         border-radius:4px;padding:4px 12px;font-size:12px;cursor:pointer;\
         font-family:inherit;white-space:nowrap;"
    };
    util::set_attr(&btn, "style", style);
    ctx.listen(&btn, "click", {
        let el = btn.clone();
        move |_evt: web_sys::Event| {
            if let Some(win) = web_sys::window() {
                let promise = win.navigator().clipboard().write_text(&link);
                // MUST consume the promise: clipboard access can be denied
                // (no focus / insecure context / permission), and a DROPPED
                // rejected promise surfaces as an `unhandledrejection` — which
                // index.html's WASM-load-failure guard treats as a reason to
                // `location.reload()`, nuking the whole session. Awaiting it in
                // a spawned task handles both outcomes so nothing leaks.
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                });
            }
            // Feedback regardless of clipboard success (it may be denied).
            el.set_text_content(Some(&crate::i18n::t("status.copied", &[])));
        }
    });
    util::append(bar, &btn);
}

/// The overlay's "Enter Peer" control (dispatches [`Action::ToggleSiteMode`] —
/// leaves the site overlay for the peer's chrome).
/// `block` = a full-width mobile-menu row; otherwise an inline desktop button.
/// Shown only when the deployment exposes the chrome↔site toggle — a
/// locked/strict-site deployment renders none, so it can't strand the user in
/// chrome (BUG-1).
fn exit_button(parent: &Element, ctx: &DomCtx, block: bool) {
    let exit = util::create_element("button");
    util::set_attr(&exit, "type", "button");
    // "Enter Peer", not "Exit Site": leaving the site overlay drops you into the
    // peer's own chrome (windows/app view) — you're not leaving, you're entering
    // the peer. Naming it after the destination is the obvious thing (the old
    // "Exit Site" read as "leave", which confused users who were still here).
    util::set_text(&exit, &crate::i18n::t("contentsite.enter_peer", &[]));
    let style = if block {
        "display:block;width:100%;text-align:start;\
         background:var(--site-exit-bg, #2a2a4e);color:var(--site-exit-text, #c0c0e0);\
         border:1px solid var(--site-exit-border, #555);border-radius:4px;\
         padding:8px 10px;font-size:13px;cursor:pointer;font-family:inherit;"
    } else {
        "background:var(--site-exit-bg, #2a2a4e);color:var(--site-exit-text, #c0c0e0);\
         border:1px solid var(--site-exit-border, #555);border-radius:4px;\
         padding:4px 12px;font-size:12px;cursor:pointer;font-family:inherit;\
         white-space:nowrap;"
    };
    util::set_attr(&exit, "style", style);
    ctx.on_action(&exit, "click", Action::ToggleSiteMode);
    util::append(parent, &exit);
}

/// Render the breadcrumb trail above the content pane. No-op when the
/// trail is empty (the root page). The trail width aligns with the
/// content pane below it.
fn render_breadcrumbs(main: &Element, output: &SiteRenderOutput, ctx: &DomCtx, host: SiteNavHost) {
    if output.breadcrumbs.is_empty() {
        return;
    }
    let trail = util::create_element("nav");
    util::set_attr(
        &trail,
        "style",
        "display:flex;flex-wrap:wrap;align-items:center;gap:6px;\
         max-width:720px;margin:0 auto;padding:14px 22px 0;font-size:12px;",
    );
    let last = output.breadcrumbs.len().saturating_sub(1);
    for (i, crumb) in output.breadcrumbs.iter().enumerate() {
        if i > 0 {
            let sep = util::create_element("span");
            util::set_text(&sep, "/");
            util::set_attr(&sep, "style", "color:var(--site-text-faint, #454a59);");
            util::append(&trail, &sep);
        }
        match &crumb.target {
            Some(target) => {
                let a = util::create_element("a");
                util::set_text(&a, &crumb.label);
                util::set_attr(&a, "href", "#");
                util::set_attr(&a, "style", "color:var(--site-link, #a6c0de);text-decoration:none;");
                wire_nav(ctx, &a, target.clone(), host);
                util::append(&trail, &a);
            }
            None => {
                let span = util::create_element("span");
                util::set_text(&span, &crumb.label);
                // The current page (last crumb) is emphasized; intermediate
                // segments are muted labels.
                let style = if i == last {
                    "color:var(--site-bc-current, #cdd3df);"
                } else {
                    "color:var(--site-text-muted-2, #7a8294);"
                };
                util::set_attr(&span, "style", style);
                util::append(&trail, &span);
            }
        }
    }
    util::append(main, &trail);
}

/// Render the left sidebar — the tree-driven section nav (the model's
/// `.list`-derived `output.sidebar`). Top-level entries are headers;
/// the active section's child pages are indented beneath it. Active
/// entries (the current page / its section trail) are highlighted.
fn render_sidebar(body_row: &Element, output: &SiteRenderOutput, ctx: &DomCtx, host: SiteNavHost) {
    // `.cs-sidebar` is a fixed side column on desktop; on mobile it stacks and
    // collapses behind the "Contents" toggle (see `RESPONSIVE_CSS`). The toggle
    // is `display:none` on desktop, so the list always shows there.
    let side = util::create_element_with_class("nav", "cs-sidebar");

    let toggle = util::create_element_with_class("button", "cs-sidebar-toggle");
    util::set_attr(&toggle, "type", "button");
    util::set_text(&toggle, &crate::i18n::t("contentsite.contents_menu", &[]));
    // Toggle the `cs-open` class (the only mobile collapse state — desktop
    // ignores it; the media query shows the list regardless there). DOM-held
    // state: survives idle frames, resets on the next rebuild (a navigation).
    ctx.listen(&toggle, "click", {
        let side = side.clone();
        move |evt: web_sys::Event| {
            evt.stop_propagation();
            let _ = side.class_list().toggle("cs-open");
        }
    });
    util::append(&side, &toggle);

    let list = util::create_element_with_class("div", "cs-sidebar-list");
    for entry in &output.sidebar {
        util::append(&list, &sidebar_link(ctx, entry, host));
    }
    util::append(&side, &list);

    util::append(body_row, &side);
}

/// One sidebar entry: a nav-wired link, indented by depth, weighted as a
/// header at depth 0, highlighted when active.
fn sidebar_link(ctx: &DomCtx, entry: &SectionLink, host: SiteNavHost) -> Element {
    let a = util::create_element("a");
    util::set_text(&a, &entry.label);
    util::set_attr(&a, "href", "#");
    let indent = if entry.depth >= 1 { "margin-inline-start:12px;" } else { "" };
    let color = if entry.active {
        "color:var(--site-accent, #9fd0ff);"
    } else if entry.depth == 0 {
        "color:var(--site-text-strong, #c3c9d6);"
    } else {
        "color:var(--site-text-muted, #9aa3b2);"
    };
    let weight = if entry.depth == 0 { "font-weight:600;" } else { "" };
    util::set_attr(
        &a,
        "style",
        &format!(
            "display:block;padding:3px 6px;text-decoration:none;\
             font-size:13px;border-radius:4px;{indent}{weight}{color}"
        ),
    );
    wire_nav(ctx, &a, entry.target.clone(), host);
    a
}

fn render_content(
    wrapper: &Element,
    output: &SiteRenderOutput,
    ctx: &DomCtx,
    host: SiteNavHost,
    resolve_asset: &AssetResolver,
) {
    // A document is laid out differently from markup, and the difference is
    // not cosmetic. Our reading column caps content at 720px — right for
    // markdown we styled ourselves, and wrong for a document that **already
    // carries its own column width** (a Pandoc artifact centers `main` at
    // 42rem). Nesting the two squeezes a 672px column into what is left of
    // 720px after 22px of padding each side, and the document reads cramped.
    // So a document pane is full-bleed and unpadded, and the frame *fills* the
    // remaining height rather than guessing a `vh` that is wrong in a window.
    let is_document = output.error.is_none() && !output.loading && output.body.is_document();
    let pane = if is_document {
        let _ = wrapper.class_list().add_1("cs-main-doc");
        util::create_element_with_class("div", "cs-pane-doc")
    } else {
        let p = util::create_element("div");
        util::set_attr(
            &p,
            "style",
            "max-width:720px;width:100%;margin:0 auto;padding:28px 22px;\
             line-height:1.6;color:var(--site-text, #e2e2ea);",
        );
        p
    };

    if let Some(err) = &output.error {
        let e = util::create_element("div");
        util::set_attr(
            &e,
            "style",
            "color:var(--site-error-text, #ff9b9b);padding:14px 16px;\
             border:1px solid var(--site-error-border, #553333);\
             border-radius:6px;background:var(--site-error-bg, #1c1418);",
        );
        util::set_text(&e, err);
        util::append(&pane, &e);
    } else if output.loading {
        let l = util::create_element("div");
        util::set_text(&l, &crate::i18n::t("status.loading", &[]));
        util::set_attr(&l, "style", "color:var(--site-text-muted, #9aa3b2);");
        util::append(&pane, &l);
    } else {
        match &output.body {
            PageRender::Markup(html) => {
                let body = util::create_element_with_class("div", "cs-doc");
                body.set_inner_html(html);
                rewrite_links(&body, output, ctx, host);
                rewrite_images(&body, resolve_asset);
                util::append(&pane, &body);
            }
            // An untrusted `format:html` document. It never touches our
            // document — neither rewriter runs, by construction: they operate
            // on elements, and there are no elements of ours to operate on.
            PageRender::Document(doc) => {
                render_document_frame(ctx, &pane, doc, &output.page_title)
            }
        }
    }

    util::append(wrapper, &pane);
}

/// The sandbox tier for an untrusted content document.
///
/// **`allow-same-origin` AND NOTHING ELSE. The token that must never join it is
/// `allow-scripts`** — the two together are strictly worse than either alone,
/// because a scripted same-origin frame can reach `parent.document`, i.e. every
/// publisher on the network gets our origin. Alone, `allow-same-origin` grants
/// an origin to a document that has **no way to use it**: scripts do not run,
/// so nothing in the frame can read a cookie, touch storage, or see our DOM.
/// Verified, not assumed — a `<script>` in the demo document stays inert at
/// this tier and Phase 19-doc reads the rendered text to prove it.
///
/// **Why the empty string is not what ships, having been what shipped.** It
/// was, for exactly one commit, and it was the right tier delivered the wrong
/// way — see [`mount_document_frame`]. `data:` at `sandbox=""` is the strictest
/// combination that navigates, and Chrome caps a `data:` URL at **2 MiB**,
/// silently: past that the frame renders *nothing* and raises *nothing*. The
/// published corpus book is 7.9 MB (10.9 MB base64), so the exact artifact this
/// tier exists to carry came up blank in one of our two browser engines. A
/// sandbox token that is unusable without a second token is a smaller price
/// than a feature that does not work.
///
/// The tiers we run, so the difference is visible in one place:
/// - `"allow-same-origin"` — **here**. Passive document, no execution.
/// - `"allow-scripts"` — `dom::games` app bundle. Opaque origin, JS runs,
///   `postMessage` is the only channel.
/// - `"allow-scripts allow-same-origin"` — an L5 app, *our own* payload.
///
/// Note what that list makes obvious: **this tier and the app tier are now one
/// token apart in each direction**, which is why Phase 19-doc asserts this
/// string exactly and why the assertion is worth keeping expensive company.
///
/// **The end state that removes the trade-off entirely** is a document served
/// from a real same-origin URL (a service-worker route or Tauri's asset
/// protocol): measured, an `http` URL at `sandbox=""` renders the 7.9 MB book
/// in both engines *and* keeps its anchors, so it is the only delivery with no
/// ceiling and no token. It is a bigger change than this file — a route, a
/// cache lifetime, and a second answer for the Tauri WebView — and it is the
/// right thing to build if this tier ever needs to be tightened again.
///
/// **KNOWN LIMITATION, measured rather than assumed: an external link inside a
/// document REPLACES the document with that site.** The earlier note here said
/// such a click was "a silent dead click" — that was wrong in the direction
/// that matters. A sandbox blocks *top-level* navigation; navigating the frame
/// **itself** is never sandboxed, so `<a href="https://example.com/">` with no
/// `target` loads example.com in place of the paper (measured in Firefox 149
/// and Chrome, every delivery tier). Only `target="_blank"` is blocked, and
/// that is what `allow-popups allow-popups-to-escape-sandbox` would enable.
/// It matters for papers, whose citations are external URLs; recovering is
/// re-navigating to the page from our own chrome.
///
/// Widening to `allow-popups` is defensible — a scriptless document can only
/// open one on a real user gesture — but it is a **security-tier decision**,
/// and making it inside a commit whose job was "render the document" is how
/// tiers drift. It also only helps if the publisher emits `target="_blank"`,
/// which is a papers-side pandoc filter: neither half works alone.
///
/// (The publisher half **has landed** — papers emit `target="_blank"
/// rel="noopener"` on all external links as of 2026-08-20 — so this is now a
/// one-sided decision on our side alone.)
const DOCUMENT_SANDBOX: &str = "allow-same-origin";

/// Mount the document from a `blob:` URL of its own, and revoke that URL as
/// soon as the frame has loaded it.
///
/// **The document needs a base URL of its own, and that is the whole point of
/// not using `srcdoc`.** A `srcdoc` document inherits the *parent's* base URL,
/// so `href="#section"` does not resolve to a fragment of the paper — it
/// resolves to `…/index.html#section`, a **different document**, and the frame
/// navigates there. A Pandoc paper's entire navigation is anchors (the
/// "Contents" list; every chapter jump in a one-file book), so before this the
/// document rendered beautifully and could not be read past the first screen.
///
/// **Measured across Firefox 149 and Chrome, on the REAL 7.9 MB corpus book**
/// (1408 fragment anchors, 475 MathML nodes, 9 inlined figures):
///
/// | delivery | sandbox | anchors | 7.9 MB book |
/// |---|---|---|---|
/// | `srcdoc` | `""` | **destroys the document** | renders |
/// | `blob:` | `""` | **inert — nothing happens** | renders |
/// | `data:` | `""` | works | **BLANK in Chrome** |
/// | **`blob:`** | **`"allow-same-origin"`** | **works** | **renders in both** |
/// | `http` URL | `""` | works | renders in both |
///
/// **`data:` shipped first and was wrong, for a reason no small fixture could
/// show: Chrome caps a `data:` URL at 2 MiB.** Bisected — 1,398,856 bytes
/// renders, 2,098,624 does not — and past the cap the frame is simply *empty*:
/// no error, no event, nothing to log. The corpus book is 10.9 MB base64, and
/// the figure-heavy single papers (06, 11, 12) are over the cap too, so the
/// artifacts this tier exists for were the ones that failed. A ceiling that
/// only the real payload crosses is exactly the shape a 500-byte demo document
/// cannot catch.
///
/// **The revoke is what makes `blob:` affordable** (D9 accounting). This
/// surface rebuilds on a subscription tick, so an unrevoked URL would pin a
/// whole book in memory *per rebuild*. Revoking on the frame's `load` event
/// bounds the lifetime to the load itself, and — measured in both engines,
/// because "should" is not evidence — **fragment navigation still works after
/// the URL is revoked**: a jump is a same-document navigation and refetches
/// nothing. A frame that never loads leaks one URL; that is the bounded case,
/// and it is the one we can live with.
///
/// **The blob MUST carry `type: "text/html"`.** Without it the frame renders
/// the book as plain text — a failure that looks like a rendering bug in the
/// document rather than a missing property bag here.
fn mount_document_blob(ctx: &DomCtx, frame: &Element, doc: &str) {
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(doc));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("text/html");
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        tracing::error!("content-site: could not build the document blob");
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        tracing::error!("content-site: could not create the document object URL");
        return;
    };

    // The revoke rides `DomCtx.listen`, so the closure lives in `ctx.closures`
    // and is freed on the next rebuild like every other handler here — never
    // `Closure::forget`, which would trade a bounded blob leak for an unbounded
    // closure one.
    let revoke_url = url.clone();
    ctx.listen(frame, "load", move |_| {
        let _ = web_sys::Url::revoke_object_url(&revoke_url);
    });

    util::set_attr(frame, "src", &url);
}

/// Mount an untrusted HTML document in a fully-restricted frame.
///
/// **D13 — the failure this exists to make visible.** A frame that renders
/// nothing looks exactly like a frame that rendered a blank page, and neither
/// raises an error. So the two states are separated *before* the mount, on the
/// one signal we do have — whether there are bytes at all — and an empty
/// document gets a message rather than an empty rectangle.
///
/// **That D13 note was written about the wrong failure, and the right one then
/// happened.** "A frame that renders nothing raises nothing" was true of an
/// *empty* document, which this guard covers; it was equally true of a document
/// past Chrome's 2 MiB `data:` cap, which this guard does not see, because the
/// bytes were all there. The guard tests the input; the ceiling was a property
/// of the delivery. **When a note says a failure is invisible, the useful
/// question is which *other* causes produce the same invisible outcome** — here
/// it was the one that mattered for every real book.
fn render_document_frame(ctx: &DomCtx, pane: &Element, doc: &str, title: &str) {
    if doc.trim().is_empty() {
        let empty = util::create_element("div");
        util::set_attr(&empty, "style", "color:var(--site-text-muted, #9aa3b2);");
        util::set_text(&empty, &crate::i18n::t("contentsite.document_empty", &[]));
        util::append(pane, &empty);
        return;
    }

    tracing::info!(bytes = doc.len(), "content-site: mounting sandboxed document frame");

    // The frame is a plain viewport (`.cs-docframe`) — no border, no radius, no
    // background, and deliberately **no `--site-*` token anywhere on it**. The
    // document supplies all its own furniture, and our theme does not reach into
    // it: a themed frame around unthemed content advertises a relationship that
    // does not exist. The document's own `background` paints the surface, so
    // setting one here would only decide the pre-paint flash.
    let frame = util::create_element_with_class("iframe", "cs-docframe");
    util::set_attr(&frame, "sandbox", DOCUMENT_SANDBOX);
    util::set_attr(&frame, "title", title);
    // A `blob:` URL on `src`, NOT `srcdoc` — see [`mount_document_blob`]. The
    // document needs a base URL of its own or its own table of contents
    // navigates the frame away from it.
    mount_document_blob(ctx, &frame, doc);
    util::append(pane, &frame);
}

/// Rewrite the mounted markdown's `<a href>` links: in-system links
/// become in-app navigation; external links open in a new tab.
fn rewrite_links(body: &Element, output: &SiteRenderOutput, ctx: &DomCtx, host: SiteNavHost) {
    let current = Location {
        peer_id: output.peer.clone(),
        site_id: output.site_id.clone(),
        page: output.current_page.clone(),
    };

    let anchors = match body.query_selector_all("a[href]") {
        Ok(n) => n,
        Err(_) => return,
    };
    for i in 0..anchors.length() {
        let Some(node) = anchors.item(i) else { continue };
        let Ok(a) = node.dyn_into::<Element>() else { continue };
        let href = a.get_attribute("href").unwrap_or_default();
        match classify_link(&href, &current) {
            LinkTarget::External { .. } => {
                a.set_attribute("target", "_blank").ok();
                a.set_attribute("rel", "noopener noreferrer").ok();
                // Leave default navigation — these leave the system.
            }
            _ => wire_nav(ctx, &a, href, host),
        }
    }
}

/// Resolve the mounted page's `<img>` sources against the site's asset
/// subgraph. Each `src` is an embed `ref` (`assets/figures/x.png`, left there
/// by [`crate::content_site::render`]); we replace it with an inline `data:`
/// URL built from the asset bytes. A ref that doesn't resolve to a site-local
/// asset (external URL, missing, wrong type) has its `src` **removed** — so the
/// browser never fetches an off-site/404 URL (the image degrades to its `alt`
/// text). This is the image analogue of [`rewrite_links`]: the renderer emits a
/// neutral ref, the DOM layer binds it to real content.
fn rewrite_images(body: &Element, resolve_asset: &AssetResolver) {
    let imgs = match body.query_selector_all("img[src]") {
        Ok(n) => n,
        Err(_) => return,
    };
    for i in 0..imgs.length() {
        let Some(node) = imgs.item(i) else { continue };
        let Ok(img) = node.dyn_into::<Element>() else { continue };
        let reference = img.get_attribute("src").unwrap_or_default();
        match resolve_asset(&reference) {
            Some((media_type, bytes)) => {
                img.set_attribute("src", &data_url(&media_type, &bytes)).ok();
                // Keep images from blowing out the content column.
                if img.get_attribute("style").is_none() {
                    img.set_attribute("style", "max-width:100%;height:auto;").ok();
                }
            }
            None => {
                // Unresolved/external — never let the browser fetch it.
                img.remove_attribute("src").ok();
            }
        }
    }
}

/// Build an inline `data:` URL from an asset's media type + bytes.
fn data_url(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{};base64,{}", media_type, crate::content_site::embed::base64_encode(bytes))
}

/// Attach a click handler that suppresses default navigation and
/// dispatches the host's navigation action ([`Action::SiteNavigate`] for
/// a window, [`Action::SiteOverlayNavigate`] for the overlay).
fn wire_nav(ctx: &DomCtx, el: &Element, target: String, host: SiteNavHost) {
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    ctx.listen(el, "click", move |evt: web_sys::Event| {
        evt.prevent_default();
        actions.borrow_mut().push(host.nav_action(target.clone()));
        rp();
    });
}

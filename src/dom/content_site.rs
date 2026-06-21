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
use crate::content_site::{classify_link, paths, LinkTarget, Location};
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
.cs-doc a{color:var(--site-link, #a6c0de);}\
.cs-doc a:hover{text-decoration:underline;}\
.cs-sidebar{flex-shrink:0;width:210px;overflow:auto;padding:18px 12px;\
border-right:1px solid var(--site-border, #20202e);\
background:var(--site-sidebar-bg, #13131c);display:flex;\
flex-direction:column;gap:2px;}\
.cs-sidebar-toggle{display:none;}\
.cs-sidebar-list{display:flex;flex-direction:column;gap:2px;}\
@media (max-width:768px){\
.cs-nav-desktop{display:none;}\
.cs-nav-burger{display:flex;align-items:center;justify-content:center;\
margin-left:auto;flex-shrink:0;background:var(--site-control-bg, #22223a);\
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
.cs-sidebar{width:auto;overflow:visible;padding:8px 12px;border-right:none;\
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
border-right:1px solid var(--site-border, #20202e);\
background:var(--site-rail-bg, #0d0d14);display:flex;\
flex-direction:column;gap:3px;}\
.cs-rail-toggle{display:none;}\
.cs-rail-list{display:flex;flex-direction:column;gap:3px;}\
@media (max-width:768px){\
.cs-window-row{flex-direction:column;overflow:auto;}\
.cs-rail{width:auto;overflow:visible;padding:8px 10px;border-right:none;\
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

    // Responsive layout rules (root-scoped; see `RESPONSIVE_CSS`).
    let style = util::create_element("style");
    util::set_text(&style, RESPONSIVE_CSS);
    util::append(container, &style);

    let wrapper = util::create_element("div");
    util::set_attr(
        &wrapper,
        "style",
        "display:flex;flex-direction:column;height:100%;overflow:hidden;\
         background:var(--site-bg, #101018);\
         font-family:system-ui,-apple-system,sans-serif;",
    );

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
///   Exit) pinned via `margin-left:auto`.
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
        util::set_attr(&back, "title", "Back");
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

    // The site title doubles as the Home button — clicking it navigates to the
    // site root (`"/"` resolves to the manifest home page). A "⌂" glyph signals
    // the affordance; it ellipsizes rather than overflow a narrow bar.
    let home = util::create_element("a");
    util::set_text(&home, &format!("\u{2302}  {}", output.site_title));
    util::set_attr(&home, "href", "#");
    util::set_attr(&home, "title", "Go to site home");
    util::set_attr(
        &home,
        "style",
        "color:var(--site-control-text, #cfe3ff);font-size:15px;font-weight:600;\
         text-decoration:none;white-space:nowrap;cursor:pointer;overflow:hidden;\
         text-overflow:ellipsis;",
    );
    wire_nav(ctx, &home, "/".to_string(), host);
    util::append(&left, &home);
    util::append(&bar, &left);

    // -- Desktop region: inline items + "More ▾" + Share/Exit right cluster --
    let desktop = util::create_element_with_class("div", "cs-nav-desktop");
    render_nav_items(&desktop, &output.nav, ctx, host);
    let right = util::create_element("div");
    util::set_attr(
        &right,
        "style",
        "display:flex;align-items:center;gap:10px;flex-shrink:0;margin-left:auto;",
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
    util::set_attr(&burger, "title", "Menu");
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
    util::set_text(&btn, &format!("More \u{25be} ({})", overflow.len()));
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
    const PANEL_BASE: &str = "position:absolute;top:calc(100% + 6px);left:0;\
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
/// published anywhere the app knows — `make publish`/`publish-serve` exports a
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
        "Share link \u{1f517}",
        "Copy a link that re-opens this page in the live entity browser",
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
        "display:block;width:100%;text-align:left;\
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
            el.set_text_content(Some("Copied \u{2713}"));
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
    util::set_text(&exit, "Enter Peer");
    let style = if block {
        "display:block;width:100%;text-align:left;\
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
    util::set_text(&toggle, "Contents \u{25be}");
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
    let indent = if entry.depth >= 1 { "margin-left:12px;" } else { "" };
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
    let pane = util::create_element("div");
    util::set_attr(
        &pane,
        "style",
        "max-width:720px;width:100%;margin:0 auto;padding:28px 22px;\
         line-height:1.6;color:var(--site-text, #e2e2ea);",
    );

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
        util::set_text(&l, "Loading…");
        util::set_attr(&l, "style", "color:var(--site-text-muted, #9aa3b2);");
        util::append(&pane, &l);
    } else {
        let body = util::create_element_with_class("div", "cs-doc");
        body.set_inner_html(&output.body_html);
        rewrite_links(&body, output, ctx, host);
        rewrite_images(&body, resolve_asset);
        util::append(&pane, &body);
    }

    util::append(wrapper, &pane);
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

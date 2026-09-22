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
    use crate::content_site::asset_store;
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
        // Inline is a field read; a `pointer` walks the blob's closure out of
        // the content store (content-site §4's `[MUST]` — every asset over
        // 16 KiB, which on the papers sites is most figures).
        //
        // **The failure is reported, not swallowed, and the reasons are kept
        // apart.** A blob we do not hold and a blob that does not decode send
        // a reader to different places, and this returning a bare `None` for
        // both is how a caching gap and a corrupt closure become one
        // indistinguishable blank image. The renderer still draws nothing
        // either way — it has one thing to draw — but the log says which.
        match asset_store::resolve(&asset, |h| peers.content_by_hash(&selector, h)) {
            Ok(bytes) => Some((asset.media_type, bytes)),
            Err(why) => {
                tracing::warn!(
                    site = %site_id,
                    asset = %name,
                    %why,
                    "site asset did not resolve — the image is left unrendered"
                );
                None
            }
        }
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
@supports (container-type:inline-size){\
.cs-main{container-type:inline-size;}\
}\
.cs-zoom{position:absolute;inset:0;z-index:90;display:none;\
flex-direction:column;gap:10px;\
background:rgba(0,0,0,0.86);padding:24px;}\
.cs-zoom.cs-open{display:flex;}\
.cs-zoom-bar{flex:0 0 auto;display:flex;justify-content:flex-end;}\
.cs-zoom-pane{flex:1 1 auto;min-height:0;display:flex;overflow:auto;}\
.cs-zoom-pane img{margin:auto;display:block;width:auto;height:auto;\
max-width:100%;max-height:100%;cursor:zoom-in;}\
.cs-zoom.cs-natural .cs-zoom-pane img{max-width:none;max-height:none;\
cursor:zoom-out;}\
.cs-zoom-close{\
background:var(--site-control-bg, #22223a);color:var(--site-control-text, #cfe3ff);\
border:1px solid var(--site-control-border, #3a3a52);border-radius:6px;\
padding:6px 12px;font-family:inherit;font-size:13px;cursor:pointer;}\
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
            "{}{}{}",
            RESPONSIVE_CSS,
            crate::content_site::doc_css::doc_css(
                ".cs-doc",
                crate::content_site::doc_css::PaletteMode::Live
            ),
            // A figure may exceed the prose column, bounded by the SCROLLING
            // PANE (`.cs-main`) and never by the viewport: this surface renders
            // inside an app window, so `100vw` is the screen and would put a
            // figure far outside the window it lives in. `cqw` is the only unit
            // that says "the box I am actually in", hence the container above.
            //
            // The whole block is `@supports`-guarded so a browser without
            // container queries gets no rule at all and keeps today's behaviour
            // — rather than a `max-width` that is invalid at computed-value
            // time, which resolves to `none` and would let a figure overflow a
            // narrow pane on exactly the browsers least able to cope.
            //
            // 94cqw leaves a gutter so the centred breakout cannot reach the
            // pane's edges; the 1400px ceiling is where a diagram stops gaining
            // from more room and starts being a wall.
            format!(
                "@supports (container-type:inline-size){{{}}}",
                crate::content_site::doc_css::figure_css(".cs-doc", "min(1400px, 94cqw)")
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
    // `position:relative` is load-bearing for the zoom overlay: it makes this
    // wrapper the containing block, so the overlay covers **the site surface**
    // and not the screen. `position:fixed` would be measured against the
    // viewport, which in a Content Site *window* means a zoom escaping its own
    // window and covering the whole desktop. The wrapper's `overflow:hidden`
    // then clips it to the surface for free.
    let mut wrapper_style = String::from(
        "position:relative;\
         display:flex;flex-direction:column;height:100%;overflow:hidden;\
         background:var(--site-bg, #101018);\
         font-family:system-ui,-apple-system,sans-serif;",
    );
    if let Some(vars) = &output.site_theme_css {
        wrapper_style.push_str(vars);
    }
    util::set_attr(&wrapper, "style", &wrapper_style);

    // Built before the content so `render_content` can hand each figure a
    // handle to it, and appended last so it sits above the page it covers.
    let zoom = build_zoom_overlay(ctx);

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
    render_content(&main, output, ctx, host, resolve_asset, &zoom);
    util::append(&body_row, &main);

    util::append(&wrapper, &body_row);
    // Last, so it paints over the page. Dropped with the wrapper on every
    // rebuild, so a zoom left open never survives a navigation.
    util::append(&wrapper, &zoom);
    util::append(container, &wrapper);
}

/// The click-to-expand overlay — `entity-core-papers`' ask 2, live half.
///
/// **Why an overlay and not a link.** On a published page a figure is wrapped
/// in an anchor to its own asset and that is the whole feature. Here the `src`
/// is a `data:` URL built from bytes in the content store, and browsers refuse
/// top-level navigation to `data:` — so the same link would be a control that
/// looks right and does nothing, on the surface the report came from.
///
/// **It opens FITTED, and a tap goes to natural size.** The first cut opened at
/// natural size and scrolled, reasoning that fitting "would reproduce the defect
/// this whole arc exists to fix, one box smaller". That reasoning holds for a
/// figure rendered *inline in prose* — scaled down with no recourse — and does
/// not transfer to an overlay, where the reader has just asked to see the thing
/// and can ask for more. On a desktop it was invisible, because a figure's
/// natural size is about the size of the surface. **On a phone it is never**:
/// measured 2026-09-19 at a 488x561 surface against `entity-core-papers`' own
/// views, opening at natural size put the reader in the **top-left corner** of
/// the drawing with **2.6%** of a 4601x2060 figure on screen (11% of
/// `ssa-overlay-entity`, 22% of `entity-topology`) — and since the app shell
/// also carried `user-scalable=no`, pinch could not get them out. That reads
/// exactly as the report did: *"it looks like we would display the image here,
/// but we didn't"*, because the corner of a graphviz drawing is margin.
///
/// So: fitted is the entry state and never upscales (`max-width`/`max-height`
/// only ever clamp down, the same "do not scale a figure away from its own
/// size" rule the inline figure keeps), and a tap on the image switches to
/// natural size **anchored on the point that was tapped** — landing back at the
/// top-left corner is the defect, not the fix.
///
/// Three ways out (a reader who cannot close a full-surface overlay is stuck):
/// the backdrop, the close button, and Escape.
fn build_zoom_overlay(ctx: &DomCtx) -> Element {
    let zoom = util::create_element_with_class("div", "cs-zoom");
    // Focusable so it can hear Escape without a document-level listener, which
    // would outlive this render and fire for a surface that is no longer here.
    util::set_attr(&zoom, "tabindex", "-1");
    util::set_attr(&zoom, "role", "dialog");
    util::set_attr(&zoom, "aria-modal", "true");

    // The button sits OUTSIDE the scroll pane. It used to be `position:sticky`
    // inside it, which both ate flow height the fit had to know about and put a
    // control over the top-right of the figure.
    let bar = util::create_element_with_class("div", "cs-zoom-bar");
    let close = util::create_element_with_class("button", "cs-zoom-close");
    util::set_text(&close, &crate::i18n::t("btn.close", &[]));
    util::set_attr(&close, "type", "button");
    util::append(&bar, &close);
    util::append(&zoom, &bar);

    // `margin:auto` on the image rather than `justify-content:center` on the
    // pane: auto margins centre a figure that fits and are treated as zero when
    // the free space is negative (flexbox §8.1), so a natural-size figure stays
    // flush at the start and **every part of it is reachable by scrolling**.
    // Centred overflow is the classic version of this that clips the left edge
    // irrecoverably — the same trap `figure_css` documents for its breakout.
    let pane = util::create_element_with_class("div", "cs-zoom-pane");
    let img = util::create_element("img");
    util::append(&pane, &img);
    util::append(&zoom, &pane);

    // The backdrop closes; the image does not, or a tap meant to zoom would
    // dismiss the thing the reader just opened.
    let z = zoom.clone();
    ctx.listen(&zoom, "click", move |_| close_zoom(&z));
    let z = zoom.clone();
    ctx.listen(&close, "click", move |evt: web_sys::Event| {
        evt.stop_propagation();
        close_zoom(&z);
    });
    let z = zoom.clone();
    ctx.listen(&img, "click", move |evt: web_sys::Event| {
        evt.stop_propagation();
        toggle_natural(&z, &evt);
    });
    let z = zoom.clone();
    ctx.listen(&zoom, "keydown", move |evt: web_sys::Event| {
        let Ok(k) = evt.dyn_into::<web_sys::KeyboardEvent>() else { return };
        if k.key() == "Escape" {
            close_zoom(&z);
        }
    });
    zoom
}

/// Fitted ⇄ natural, **anchored on the point that was tapped**.
///
/// Going to natural size without an anchor drops the reader at the top-left
/// corner, which for a 4601px-wide figure is the defect this function exists to
/// fix wearing a control's clothes. So: take the tapped point as a fraction of
/// the fitted image, switch, and scroll that fraction to the centre of the pane.
/// Reading a layout property after the class change is what forces the new
/// geometry to be current before we measure it.
fn toggle_natural(zoom: &Element, evt: &web_sys::Event) {
    let Ok(Some(pane)) = zoom.query_selector(".cs-zoom-pane") else { return };
    let Ok(Some(img)) = zoom.query_selector(".cs-zoom-pane img") else { return };
    let classes = zoom.class_list();

    if classes.contains("cs-natural") {
        let _ = classes.remove_1("cs-natural");
        pane.set_scroll_left(0);
        pane.set_scroll_top(0);
        return;
    }

    // Where in the figure did they tap? `offsetX/Y` is already relative to the
    // target, and the target IS the image, so this is the fraction directly.
    // `0.5` is the honest fallback when the event carries no coordinates (a
    // synthesized click, or a keyboard activation) — the centre, not a corner.
    let (fit_w, fit_h) = (img.client_width(), img.client_height());
    let (fx, fy) = match evt.dyn_ref::<web_sys::MouseEvent>() {
        Some(m) if fit_w > 0 && fit_h > 0 => (
            (f64::from(m.offset_x()) / f64::from(fit_w)).clamp(0.0, 1.0),
            (f64::from(m.offset_y()) / f64::from(fit_h)).clamp(0.0, 1.0),
        ),
        _ => (0.5, 0.5),
    };

    let _ = classes.add_1("cs-natural");
    // Forces layout, so the rect below is the natural-size one.
    let full_w = f64::from(pane.scroll_width());
    let full_h = f64::from(pane.scroll_height());
    let view_w = f64::from(pane.client_width());
    let view_h = f64::from(pane.client_height());
    pane.set_scroll_left((fx * full_w - view_w / 2.0).max(0.0) as i32);
    pane.set_scroll_top((fy * full_h - view_h / 2.0).max(0.0) as i32);
}

/// Show `src` in the overlay, fitted.
fn open_zoom(zoom: &Element, src: &str, alt: &str) {
    if let Ok(Some(img)) = zoom.query_selector(".cs-zoom-pane img") {
        img.set_attribute("src", src).ok();
        img.set_attribute("alt", alt).ok();
    }
    // Always FITTED on open, and scrolled to the start: the overlay element
    // outlives one open/close cycle, so a zoom and a pan left behind by the
    // last figure would otherwise greet the next one.
    let _ = zoom.class_list().remove_1("cs-natural");
    if let Ok(Some(pane)) = zoom.query_selector(".cs-zoom-pane") {
        pane.set_scroll_left(0);
        pane.set_scroll_top(0);
    }
    let _ = zoom.class_list().add_1("cs-open");
    // Focus is what makes Escape reach the listener above.
    if let Some(el) = zoom.dyn_ref::<web_sys::HtmlElement>() {
        let _ = el.focus();
    }
}

/// Hide the overlay and **release the bytes**. An inlined `data:` URL is the
/// whole asset held in an attribute; leaving it on a hidden element keeps a
/// multi-megabyte string alive for the life of the render.
fn close_zoom(zoom: &Element) {
    let _ = zoom.class_list().remove_1("cs-open");
    let _ = zoom.class_list().remove_1("cs-natural");
    if let Ok(Some(img)) = zoom.query_selector(".cs-zoom-pane img") {
        img.remove_attribute("src").ok();
    }
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

/// Append the share control — a single **"🔗 Share link"** copying the `?site=`
/// deep link that re-opens *this* page in the live entity browser.
///
/// **It carries the concrete publisher peer-id, not the `self` sentinel** — see
/// [`paths::share_deep_link`], which owns that choice and the measurement behind
/// it. Emitting `self` here was a real defect: `self` resolves to the *reader's*
/// own booting peer, so every link copied out of a published domain reported
/// *"No site manifest"* at a peer-id that differed per visitor.
///
/// **The "why no static link" note that stood here is spent.** It said a static
/// permalink was unavailable until *"the hosting-identity piece: the live peer
/// publishing its OWN tree, or a registry telling the app where this peer's site
/// is published."* Per-domain publishing gave us durable publisher identities and
/// the registry signs `name → peer-id`, so both arrived — which is exactly what
/// makes the link below expressible. A separate *static* (`/sites/{peer}/…`)
/// permalink is still not offered; the live link now works for foreign sites,
/// which is what the removed control was reaching for.
fn render_share_button(bar: &Element, output: &SiteRenderOutput, ctx: &DomCtx, block: bool) {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    let live_link = paths::share_deep_link(
        &origin,
        output.peer.as_deref(),
        &output.site_id,
        &output.current_page,
    );
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
    // **The link is a host-side observable, because otherwise nothing can gate
    // it.** It is captured by the click closure and written to the clipboard,
    // and a headless clipboard read is both awkward and permission-gated — so
    // the one property worth asserting (*does a shared link carry the
    // publisher's peer or the reader's own?*) would be untestable in a browser.
    // That is not hypothetical: emitting the `self` sentinel here shipped and
    // was found by a person, not a gate. Same move as `data-app-state-seq`.
    util::set_attr(&btn, "data-share-link", &link);
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
    zoom: &Element,
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
            // **Do NOT retire the live document URL here.** It looks like the
            // obvious place — a markup render is "the reader left the document",
            // so the book we were holding should go. It is not: this surface
            // renders markup *between* document renders, and the retire then
            // revokes the URL of the document that is still on screen. Measured,
            // with the mount and the retire both traced: `mount A · retire A ·
            // mount B · retire B`, leaving the visible frame holding a revoked
            // URL and its anchors dead after ~5 jumps. A document URL is retired
            // by the NEXT document mount and by nothing else.
            PageRender::Markup(html) => {
                let body = util::create_element_with_class("div", "cs-doc");
                body.set_inner_html(html);
                rewrite_links(&body, output, ctx, host);
                rewrite_images(&body, resolve_asset, ctx, zoom);
                util::append(&pane, &body);
            }
            // An untrusted `format:html` document. It never touches our
            // document — neither rewriter runs, by construction: they operate
            // on elements, and there are no elements of ours to operate on.
            PageRender::Document(doc) => render_document_frame(&pane, doc, &output.page_title),
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
/// - `"allow-scripts allow-downloads"` — `dom::games` app bundle. Opaque origin,
///   JS runs, `postMessage` is the only channel. The download token permits a
///   download to be *initiated* and nothing else; it does not weaken origin
///   isolation, and without it `Entity.Export` fails **silently** in every art
///   app (2026-09-11).
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

/// Mount the document from a `blob:` URL of its own, keeping that URL alive for
/// as long as the document is mounted and retiring the previous one.
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
/// **The revoke is what makes `blob:` affordable** (D9 accounting) — but it
/// must retire the document we are *replacing*, never the one we just mounted.
/// Revoking on the frame's own `load` shipped first and **broke every book after
/// a handful of anchor jumps**; the reasoning, the measurement that missed it,
/// and the numbers are in [`retire_live_document_url`]. What survives is the
/// accounting: this surface rebuilds on a subscription tick, so the invariant is
/// **exactly one live object URL at a time**, not one per rebuild.
///
/// **The blob MUST carry `type: "text/html"`.** Without it the frame renders
/// the book as plain text — a failure that looks like a rendering bug in the
/// document rather than a missing property bag here.
fn mount_document_blob(frame: &Element, doc: &str) {
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

    // Retire the PREVIOUS document's URL, never this one — see
    // [`retire_live_document_url`]. At most one book is ever held.
    retire_live_document_url();
    LIVE_DOCUMENT_URL.with(|slot| *slot.borrow_mut() = Some(url.clone()));

    util::set_attr(frame, "src", &url);
}

thread_local! {
    /// The object URL of the document currently mounted, and the only one alive.
    static LIVE_DOCUMENT_URL: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Revoke the object URL of the document we are replacing.
///
/// **REVOKING ON THE FRAME'S OWN `load` IS WHAT SHIPPED, AND IT BREAKS THE
/// DOCUMENT AFTER A HANDFUL OF ANCHOR JUMPS.** The reasoning behind it was that
/// a fragment jump is a same-document navigation and refetches nothing, so the
/// URL is dead weight the moment the frame has it. That was *measured* — and
/// measured on **one** jump, which is exactly how long it holds. Each fragment
/// navigation pushes a session-history entry, and once the browser needs the
/// URL again to service that history, a revoked one yields nothing: the hash
/// stops changing and the document simply stops responding to its own table of
/// contents, with no error anywhere.
///
/// Measured on the real 7.9 MB corpus book, clicking TOC links one at a time
/// with the frame otherwise untouched (`sandbox="allow-same-origin"`, `blob:`,
/// only the revoke varied):
///
/// | revoke | jumps that work |
/// |---|---|
/// | on the frame's `load` (shipped) | **6, then dead at `#part6`** |
/// | **on replacement (here)** | **8/8, and on to `#part8`** |
///
/// So the URL must stay alive for as long as the document is *mounted*, and the
/// accounting D9 wants is bought a different way: exactly one is ever live, and
/// mounting the next one retires the last. A surface left on a document holds
/// one book — bounded, and the same order as the rendered document itself —
/// rather than one per rebuild, which was the unbounded case the revoke existed
/// to prevent.
///
/// **The lesson this repeats, in the same feature, one layer along:** the 2 MiB
/// `data:` ceiling was invisible because the fixture never approached it. This
/// was invisible because the *interaction* never approached it — one click
/// where a reader makes twenty. When a claim is about a repeated action,
/// measure the repetition, not the first one.
///
/// **And the second retire site — the one that looked obviously right — is the
/// trap.** The first fix also retired here when the surface rendered *markup*,
/// reasoning that a markup render means the reader left the document. It does
/// not: this surface renders markup **between** document renders, so that
/// retire revoked the URL of the document still on screen and the bug survived
/// its own fix. Traced, with both sites logged: `mount A · retire A · mount B ·
/// retire B`. A document URL is retired by the NEXT document mount and by
/// nothing else.
///
/// **The gate does not cover this** — `e2e_worker` Phase 19-doc stays green
/// with the original bug reintroduced (mutation-checked). See the note there;
/// the check that works is a real book and a dozen TOC clicks.
fn retire_live_document_url() {
    LIVE_DOCUMENT_URL.with(|slot| {
        if let Some(url) = slot.borrow_mut().take() {
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    });
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
fn render_document_frame(pane: &Element, doc: &str, title: &str) {
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
    mount_document_blob(&frame, doc);
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
fn rewrite_images(body: &Element, resolve_asset: &AssetResolver, ctx: &DomCtx, zoom: &Element) {
    let imgs = match body.query_selector_all("img[src]") {
        Ok(n) => n,
        Err(_) => return,
    };
    for i in 0..imgs.length() {
        let Some(node) = imgs.item(i) else { continue };
        let Ok(img) = node.dyn_into::<Element>() else { continue };
        let reference = img.get_attribute("src").unwrap_or_default();
        // Only a FIGURE opens — a standalone image, the same `p>img:only-child`
        // the stylesheet widens. An image inside a sentence is not a figure and
        // must not acquire a control the author never wrote (the static half
        // draws the same line, in `link_figures_to_their_asset`).
        let is_figure = img.matches("p>img:only-child").unwrap_or(false);
        match resolve_asset(&reference) {
            Some((media_type, bytes)) => {
                img.set_attribute("src", &data_url(&media_type, &bytes)).ok();
                if is_figure {
                    let z = zoom.clone();
                    let target = img.clone();
                    ctx.listen(&img, "click", move |_| {
                        let src = target.get_attribute("src").unwrap_or_default();
                        let alt = target.get_attribute("alt").unwrap_or_default();
                        if !src.is_empty() {
                            open_zoom(&z, &src, &alt);
                        }
                    });
                }
                // **No inline sizing style here, deliberately.** It used to set
                // `max-width:100%;height:auto` "to keep images from blowing out
                // the content column" — which `doc_css`'s own `img` rule
                // already does, since this body is mounted as `.cs-doc`. So it
                // was redundant the day it was written and became actively
                // harmful the day a figure earned a wider rule: an inline style
                // outranks every author stylesheet, so it silently pinned every
                // figure to the prose measure on THIS surface while the
                // identical rule worked on the static export. A fix that lands
                // on one of two hosts and reports nothing on the other is the
                // shape that costs a session — the sizing contract lives in one
                // stylesheet (S-T1) and nothing may re-state it per element.
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

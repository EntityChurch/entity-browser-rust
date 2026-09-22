//! Shared inline style constants for DOM window views.
//!
//! These are applied via set_attribute("style", ...) on individual elements.
//! The Shadow DOM stylesheet (style.rs) handles class-based layout; these
//! handle widget-level styling that's shared across window views.

// --- Spacing scale (REFERENCE-UI-DESIGN S1) ---------------------------------
// One constrained scale, base 4px. Use these instead of ad-hoc pixel values so
// spacing is consistent across windows. Rule of thumb: tight WITHIN a group
// (SP_1/SP_2), loose BETWEEN groups (SP_3/SP_4).
pub const SP_1: &str = "4px";
pub const SP_2: &str = "8px";
pub const SP_3: &str = "12px";
// The larger steps are part of the published scale; consumers adopt them as
// windows migrate off ad-hoc pixel values (S1). Kept so the scale is complete.
#[allow(dead_code)]
pub const SP_4: &str = "16px";
#[allow(dead_code)]
pub const SP_5: &str = "24px";
#[allow(dead_code)]
pub const SP_6: &str = "32px";

/// Window section padding wrapper.
pub const SECTION: &str = "padding:12px";

/// Section heading (h2).
pub const HEADING: &str = "margin:0 0 8px";

/// Form label.
pub const LABEL: &str = "font-size:12px;font-weight:bold;display:block;margin-top:6px";

/// Radio/checkbox label (settings-style).
pub const LABEL_CHOICE: &str = "display:block;margin:4px 0;cursor:pointer";

/// Hint text below a form field.
pub const HINT: &str = "font-size:11px;color:var(--text-dim,#888);margin:0 0 4px 0";

/// Row holding a full identifier plus its Copy button (Peer Connections' bound
/// peer id). Wraps rather than overflowing — a Base58 peer id is long, and the
/// point of showing it in full is that all of it is readable and selectable.
/// Spacing values are the SP_2/SP_3 steps, inlined because a `const` can't
/// interpolate them.
pub const ID_ROW: &str = "display:flex;align-items:center;gap:8px;margin:0 0 12px 0;flex-wrap:wrap";

/// The identifier itself inside [`ID_ROW`]: dim, monospace-sized, breakable at
/// any character, and `user-select:all` so one click selects the whole id (the
/// thing people actually want to do with it).
pub const ID_CODE: &str =
    "font-size:11px;color:var(--text-dim,#888);word-break:break-all;user-select:all";

/// Text input field.
pub const INPUT: &str = "display:block;width:100%;background:var(--input-bg,#0e0e1e);\
    color:var(--text,#e0e0e0);border:1px solid var(--border-strong,#444);padding:4px 8px;\
    font-family:var(--font-mono,monospace);font-size:12px;\
    border-radius:3px;box-sizing:border-box;margin:2px 0 6px 0";

/// Multi-line text area (markdown/content editors). Pair with
/// `util::tracked_textarea` so typing survives rebuilds.
/// Native color-picker swatch (`<input type=color>`) — compact square sized
/// to sit beside a text input in a token-editor row.
pub const COLOR_SWATCH: &str = "width:28px;height:24px;padding:0;\
    border:1px solid var(--border-strong,#444);border-radius:3px;\
    background:var(--input-bg,#0e0e1e);cursor:pointer;flex:0 0 auto";

pub const TEXTAREA: &str = "display:block;width:100%;min-height:300px;\
    background:var(--input-bg,#0e0e1e);color:var(--text,#e0e0e0);\
    border:1px solid var(--border-strong,#444);padding:8px;font-size:13px;\
    font-family:var(--font-mono,monospace);line-height:1.5;border-radius:3px;\
    box-sizing:border-box;margin:2px 0 0 0;resize:vertical";

/// ⭐ **A short multi-line composer** — a post box, not a document editor.
///
/// [`TEXTAREA`] is `min-height:300px` because its callers are editors (the
/// Knowledge Base body, the site editor). A feed post was a **single-line
/// `INPUT`** stretched across the whole window, which is wrong twice: the body
/// is markdown, so it has line breaks, and a 1400-pixel-wide one-line box is
/// the widest possible way to show the least possible text.
///
/// `max-width` is [`READING_COLUMN`]'s number for the same reason — what you
/// type and what other people then read are the same measure.
pub const COMPOSE_BOX: &str = "display:block;width:100%;max-width:720px;min-height:78px;\
    background:var(--input-bg,#0e0e1e);color:var(--text,#e0e0e0);\
    border:1px solid var(--border-strong,#444);padding:8px;font-size:13px;\
    line-height:1.5;border-radius:3px;box-sizing:border-box;margin:2px 0 6px 0;\
    resize:vertical";

/// ⭐ **A reading column.** Prose at the full width of a maximized window is
/// unreadable — the eye loses the line on the way back. 720px is not a new
/// number: it is what the Site Browser's own document column already uses
/// (`dom::content_site`), so a post and a page measure the same.
///
/// Applied to the parts a person *reads*, never to the tables — a browse row
/// has columns and wants the width it is given.
pub const READING_COLUMN: &str = "max-width:720px";

/// Select dropdown.
pub const SELECT: &str = "display:block;width:100%;background:var(--input-bg,#0e0e1e);\
    color:var(--text,#e0e0e0);border:1px solid var(--border-strong,#444);padding:4px 8px;\
    font-size:12px;border-radius:3px;box-sizing:border-box;margin:2px 0 6px 0";

/// Primary action button (green).
pub const BTN_PRIMARY: &str = "background:var(--btn-primary-bg,#2a4a2e);\
    color:var(--accent-green,#c0e0c0);border:1px solid var(--btn-primary-border,#4a4);\
    padding:6px 16px;border-radius:3px;cursor:pointer;font-size:13px;margin:2px";

/// Secondary action button (blue).
pub const BTN_SECONDARY: &str = "background:var(--surface,#2a2a4e);\
    color:var(--accent-2,#c0c0e0);border:1px solid var(--btn-secondary-border,#66f);\
    padding:6px 16px;border-radius:3px;cursor:pointer;font-size:13px;margin:2px";

/// Small/neutral button.
pub const BTN_SMALL: &str = "background:var(--surface,#2a2a4e);color:var(--text-muted,#c0c0c0);\
    border:1px solid var(--border-strong,#444);\
    padding:4px 12px;border-radius:3px;cursor:pointer";

/// Destructive action button (Delete/Forget) — outlined in the error color,
/// never solid: a destructive action is never styled as the group's primary
/// (S3). Promoted from Site Editor's local `BTN_DANGER`, metrics harmonized
/// with the `BTN_*` family.
pub const BTN_DESTRUCTIVE: &str = "background:transparent;color:var(--status-err,#f66);\
    border:1px solid var(--status-err,#f66);\
    padding:6px 16px;border-radius:3px;cursor:pointer;font-size:13px;margin:2px";

/// Toggle button (active state).
pub const TOGGLE_ACTIVE: &str = "background:var(--surface,#2a2a4e);color:var(--text-muted,#c0c0c0);\
    border:1px solid var(--btn-secondary-border,#66f);\
    padding:4px 12px;border-radius:3px;cursor:pointer;font-size:12px";

/// Toggle button (inactive state).
pub const TOGGLE_INACTIVE: &str = "background:var(--bg,#1a1a2e);color:var(--text-dim,#888);\
    border:1px solid var(--border-strong,#444);\
    padding:4px 12px;border-radius:3px;cursor:pointer;font-size:12px";

/// Collapsible section header (the shared disclosure primitive —
/// `components::collapsible_header`). A full-width, left-aligned toggle button
/// carrying a ▾/▸ marker + label. One look for every "New/section" disclosure,
/// so Peers, Site Creator, etc. can't drift into bespoke reveals.
pub const COLLAPSIBLE_HEADER: &str = "display:flex;align-items:center;gap:8px;width:100%;\
    text-align:start;background:var(--surface-sunken,#15152a);color:var(--text,#e0e0e0);\
    border:1px solid var(--border,#2a2a4e);border-radius:6px;padding:9px 12px;margin-top:12px;\
    font-size:15px;font-weight:bold;cursor:pointer";

/// Pre-formatted output area (event log, results).
pub const PRE_OUTPUT: &str = "background:var(--surface-sunken,#0a0a1a);padding:8px;border-radius:4px;\
    font-size:11px;max-height:400px;overflow:auto;white-space:pre-wrap;margin:0";

/// Window title inside a HEADER_ROW (an h2 sharing the row with controls).
pub const TITLE_INLINE: &str = "margin:0;font-size:16px";

/// Body note inside a card — plain explanatory text (larger than HINT).
pub const NOTE: &str = "font-size:13px;margin:8px 0";

/// Right-aligned control row (e.g. a Refresh button above a listing).
pub const ROW_END: &str = "display:flex;flex-wrap:wrap;justify-content:flex-end;gap:4px;margin-bottom:8px";

/// Left-aligned control row — [`ROW_END`]'s mirror, for controls that read with
/// the content rather than against it (a filter-chip bar above a grid).
pub const ROW_START: &str = "display:flex;flex-wrap:wrap;gap:8px;margin-bottom:16px";

/// A left-aligned row of controls sharing a baseline with text beside them (a
/// back button next to a heading, a timestamp next to its two actions).
/// [`ROW_START`] with vertical centring and no bottom margin.
pub const ROW_INLINE: &str = "display:flex;flex-wrap:wrap;align-items:center;gap:8px";

/// A **full-bleed panel body** — the whole window content, scrolling, with its
/// own background. The Apps launcher grid and the Saves panel share it, so the
/// window does not change shape as you move between its views.
///
/// `min-height` (NOT `height:100%`) is the floor: a percentage height collapses
/// in an auto-height tiled window (the `.window` section is content-driven),
/// which cramps the body to the ~200px section minimum. A min-height gives it
/// real space when tiled and, via the `.window-content > *` flex-stretch, still
/// fills a maximized window.
pub const PANEL_SURFACE: &str = "min-height:480px;width:100%;overflow:auto;padding:20px;\
    box-sizing:border-box;background:var(--bg,#101018);color:var(--text,#e2e2ea);\
    font-family:system-ui,-apple-system,sans-serif";

/// The heading at the top of a [`PANEL_SURFACE`] body.
pub const PANEL_TITLE: &str = "font-size:18px;font-weight:600;margin:0 0 16px 2px";

/// A cell holding a nested detail block under the row it belongs to (an
/// expanded list inside a table). Pair with `colspan`.
pub const TD_NESTED: &str = "padding:8px 16px;border-bottom:1px solid var(--border,#333)";

/// Bounded scrolling list region (tree browsers, long listings). Pair with a
/// `data-scroll-key` attribute so scroll position survives rebuilds.
pub const SCROLL_LIST: &str = "margin:4px 0;max-height:260px;overflow:auto";

// Tree-browser rows (`components::tree_row`) — ONE look for every lazy tree
// (Site Editor navigator, File Transfer share browser). Promoted from Site
// Editor's local consts when File Transfer shipped a second bespoke copy (S8).
pub const TREE_ROW: &str = "display:flex;align-items:center;gap:4px;margin:2px 0";
pub const TREE_CARET: &str = "background:transparent;border:none;color:var(--text,#e0e0e0);\
    cursor:pointer;font-size:15px;width:22px;padding:0;line-height:1;flex:0 0 22px";
pub const TREE_NODE: &str = "flex:1 1 auto;text-align:start;background:transparent;\
    color:var(--text,#e0e0e0);border:1px solid transparent;border-radius:4px;\
    padding:3px 8px;font-size:14px;cursor:pointer;overflow:hidden;text-overflow:ellipsis";
/// One shared "selected" highlight (accent border + accent text) so the open
/// page and the selected file read the same across windows.
pub const TREE_NODE_SELECTED: &str = "flex:1 1 auto;text-align:start;background:transparent;\
    color:var(--accent,#3a6ea5);border:2px solid var(--accent,#3a6ea5);\
    border-radius:4px;padding:2px 7px;font-size:14px;font-weight:600;cursor:pointer;\
    overflow:hidden;text-overflow:ellipsis";

/// Section grouping.
#[allow(dead_code)] // layout token kept for the shared set's completeness (siblings BTN_ROW/HEADER_ROW are used)
pub const SECTION_GROUP: &str = "margin-bottom:12px";

// NOTE: every shared flex row carries `flex-wrap:wrap`. These are
// applied inline, so the responsive stylesheet CANNOT override them on
// narrow screens — without wrap, button/header rows crammed or
// overflowed on mobile across many windows. `flex-wrap:wrap` is a
// no-op when there's room and the single safe project-wide fix.

/// Button row container.
pub const BTN_ROW: &str = "margin:8px 0;display:flex;flex-wrap:wrap;gap:4px";

/// Header with space-between layout.
pub const HEADER_ROW: &str = "display:flex;flex-wrap:wrap;justify-content:space-between;align-items:center;gap:8px;margin-bottom:8px";

/// Checkbox row.
#[allow(dead_code)] // layout token kept for the shared set's completeness (siblings BTN_ROW/HEADER_ROW are used)
pub const CHECKBOX_ROW: &str = "margin:6px 0;display:flex;flex-wrap:wrap;align-items:center;gap:6px";

// --- System Monitor (DESIGN-2026-09-14-c §3) --------------------------------
//
// Text mode: panes are bordered boxes with an inset title, graphs are braille
// text in the accent colour, meters are block characters. Everything is a theme
// token, so all twelve themes get a monitor that matches.

/// The panes, flowing into as many columns as fit and one column on a phone.
pub const MONITOR_GRID: &str = "display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,300px),1fr));\
    gap:10px;margin:8px 0";

/// One pane: a box-drawing-style border with the title set into it.
pub const MONITOR_PANE: &str = "border:1px solid var(--border-strong,#3a3a5e);border-radius:4px;\
    padding:12px 10px 8px;position:relative;min-width:0;font-family:var(--font-mono,monospace);font-size:12px";

/// A pane's title, sitting on its top border.
pub const MONITOR_PANE_TITLE: &str = "position:absolute;top:-8px;inset-inline-start:8px;padding:0 4px;\
    background:var(--bg,#101018);color:var(--accent,#7aa2f7);font-size:11px;font-weight:bold;\
    text-transform:uppercase;letter-spacing:0.05em";

/// A braille graph or block meter. Monospace with braille-capable fallbacks,
/// no line gap, so rows of cells read as one picture.
///
/// **`direction:rtl` is for the overflow, not the text.** The newest samples are
/// at the right; a graph wider than its pane must lose its OLDEST end, and an
/// RTL box overflows to the left. The text inside is set back to LTR by
/// [`MONITOR_GRAPH_TEXT`], so the cells keep their order.
pub const MONITOR_GRAPH: &str = "font-family:\"DejaVu Sans Mono\",\"Noto Sans Symbols 2\",var(--font-mono,monospace);\
    font-size:14px;line-height:1;color:var(--accent,#7aa2f7);white-space:pre;overflow:hidden;letter-spacing:0;\
    direction:rtl;margin:2px 0";

/// The same, for a figure that crossed a warning line.
pub const MONITOR_GRAPH_WARN: &str = "font-family:\"DejaVu Sans Mono\",\"Noto Sans Symbols 2\",var(--font-mono,monospace);\
    font-size:14px;line-height:1;color:var(--status-warn,#e0af68);white-space:pre;overflow:hidden;letter-spacing:0;\
    direction:rtl;margin:2px 0";

/// A meter or a short fixed-width graph: never wider than its box, so it keeps
/// the reading direction and starts at the left.
pub const MONITOR_METER: &str = "font-family:\"DejaVu Sans Mono\",\"Noto Sans Symbols 2\",var(--font-mono,monospace);\
    font-size:14px;line-height:1;color:var(--accent,#7aa2f7);white-space:pre;overflow:hidden;letter-spacing:0;margin:2px 0";

/// [`MONITOR_METER`] past a warning line.
pub const MONITOR_METER_WARN: &str = "font-family:\"DejaVu Sans Mono\",\"Noto Sans Symbols 2\",var(--font-mono,monospace);\
    font-size:14px;line-height:1;color:var(--status-warn,#e0af68);white-space:pre;overflow:hidden;letter-spacing:0;margin:2px 0";

/// The graph's text inside a [`MONITOR_GRAPH`] box — back to left-to-right.
pub const MONITOR_GRAPH_TEXT: &str = "direction:ltr;unicode-bidi:isolate";

/// One labelled figure line.
pub const MONITOR_LINE: &str = "margin:4px 0 0;color:var(--text,#e2e2ea);overflow-wrap:anywhere";

/// Where a figure came from — small and dim, after the figure.
pub const MONITOR_SOURCE: &str = "color:var(--text-faint,#666);font-size:10px";

/// A window header's *running app* label, beside the title: dimmer than the
/// title, truncated rather than wrapped so the header keeps its height.
/// `margin-inline-end:auto` keeps it beside the title in a header that spaces
/// its children apart.
pub const WINDOW_APP_LABEL: &str = "color:var(--text-dim,#888);font-size:12px;margin-inline:6px auto;\
    white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0;flex:0 1 auto";

/// What a figure means, in a sentence — dimmer than the figure, full width.
pub const MONITOR_NOTE: &str = "margin:2px 0 6px;color:var(--text-dim,#888);font-size:11px;line-height:1.35;\
    font-family:system-ui,sans-serif;overflow-wrap:anywhere";

/// The tab pane's headline, by how the tab feels.
pub const MONITOR_FEEL_OK: &str = "margin:0 0 4px;color:var(--status-ok,#9ece6a);font-weight:bold;overflow-wrap:anywhere";
pub const MONITOR_FEEL_WARN: &str = "margin:0 0 4px;color:var(--status-warn,#e0af68);font-weight:bold;overflow-wrap:anywhere";
pub const MONITOR_FEEL_BAD: &str = "margin:0 0 4px;color:var(--status-err,#f7768e);font-weight:bold;overflow-wrap:anywhere";

/// A window row's Show / Close buttons.
pub const MONITOR_ROW_ACTIONS: &str = "display:flex;flex-wrap:wrap;gap:4px;justify-content:flex-end";

/// The explanation drawer: room above the button, and above the pane it opens,
/// whose title is set into its top border.
pub const MONITOR_DRAWER: &str = "margin:14px 0 0;display:flex;flex-direction:column;align-items:flex-start;gap:16px";

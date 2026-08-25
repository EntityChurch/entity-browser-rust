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

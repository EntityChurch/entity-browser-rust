//! Shared inline style constants for DOM window views.
//!
//! These are applied via set_attribute("style", ...) on individual elements.
//! The Shadow DOM stylesheet (style.rs) handles class-based layout; these
//! handle widget-level styling that's shared across window views.

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

/// Toggle button (active state).
pub const TOGGLE_ACTIVE: &str = "background:var(--surface,#2a2a4e);color:var(--text-muted,#c0c0c0);\
    border:1px solid var(--btn-secondary-border,#66f);\
    padding:4px 12px;border-radius:3px;cursor:pointer;font-size:12px";

/// Toggle button (inactive state).
pub const TOGGLE_INACTIVE: &str = "background:var(--bg,#1a1a2e);color:var(--text-dim,#888);\
    border:1px solid var(--border-strong,#444);\
    padding:4px 12px;border-radius:3px;cursor:pointer;font-size:12px";

/// Pre-formatted output area (event log, results).
pub const PRE_OUTPUT: &str = "background:var(--surface-sunken,#0a0a1a);padding:8px;border-radius:4px;\
    font-size:11px;max-height:400px;overflow:auto;white-space:pre-wrap;margin:0";

/// Section grouping.
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
pub const CHECKBOX_ROW: &str = "margin:6px 0;display:flex;flex-wrap:wrap;align-items:center;gap:6px";

//! CSS for the DOM renderer, injected into the Shadow DOM.

pub const DOM_STYLES: &str = r#"
:host {
    display: block;
    height: 100%;
    font-family: var(--font-ui, system-ui, -apple-system, sans-serif);
    font-size: 14px;
    color: var(--text, #e0e0e0);
    background: var(--bg, #1a1a2e);
    /* Inherit the document `color-scheme` (set per-theme in the :root
       block, theme_tokens::root_block) across the shadow boundary, so
       native controls inside windows render in the active scheme. */
    color-scheme: inherit;
}

/* Native form controls. The CLOSED <select> is themed inline (theme::SELECT),
   but the OPENED popup + its <option> rows are drawn natively — WebKitGTK
   (Tauri) ignores the inline style there and falls back to system chrome,
   giving unreadable pale-on-pale option text under the dark UI. Style the
   options explicitly (belt-and-suspenders alongside the :root color-scheme)
   so the popup is legible on every WebView. Chromium/Firefox were already OK. */
select {
    color-scheme: inherit;
}
select option,
select optgroup {
    background-color: var(--input-bg, #0e0e1e);
    color: var(--text, #e0e0e0);
}

.window-manager {
    display: flex;
    width: 100%;
    height: 100%;
    overflow: hidden;
    /* Containing block for a maximized window surface (inset:0 below). */
    position: relative;
}

/* Command palette — left sidebar */
.command-palette {
    width: 180px;
    min-width: 140px;
    border-right: 1px solid var(--border, #333);
    padding: 8px;
    overflow-y: auto;
    flex-shrink: 0;
}

.command-palette summary {
    cursor: pointer;
    font-size: 15px;
    font-weight: bold;
    padding: 6px 4px;
    user-select: none;
    border-radius: 3px;
}

.command-palette summary::-webkit-details-marker {
    margin-right: 6px;
}

.command-palette summary:hover {
    color: var(--accent, #90d0ff);
}

.command-palette h3 {
    margin: 10px 0 4px 0;
    font-size: 13px;
    color: var(--text-dim, #888);
}

/* Mobile-collapse wrapper. On desktop the whole palette is the sidebar, so the
   toggle bar is hidden and both panels always show. The mobile @media block
   flips this: a split bar (☰ Menu | Open Windows) appears and the two panels
   (.palette-body / .palette-windows) collapse behind their own toggles. */
.palette-bar,
.palette-toggle,
.palette-windows-toggle {
    display: none;
}

.palette-body,
.palette-windows {
    display: block;
}

/* Per-panel header bar — desktop hides it (the sidebar already has structure);
   mobile shows it to separate the menu from the open-windows list. */
.palette-panel-head {
    display: none;
}

/* Collapsible menu group (Apps & Content / System / Developer / Open Windows).
   The summary is the group header; its disclosure triangle is the affordance. */
.palette-group {
    margin-bottom: 6px;
}

.palette-group > summary {
    cursor: pointer;
    font-size: 13px;
    font-weight: bold;
    padding: 6px 4px;
    user-select: none;
    border-radius: 3px;
    color: var(--text-muted, #c0c0c0);
}

.palette-group > summary:hover {
    color: var(--accent, #90d0ff);
}

.palette-group[open] > summary {
    margin-bottom: 4px;
}

/* Games/Apps launcher cards. Inline styles set the per-card accent in
   `--app-fg`; the stylesheet owns the interactive states so a card lifts to its
   own color on hover/focus. */
.app-card {
    transition: border-color 0.12s ease, background 0.12s ease, transform 0.06s ease;
}

.app-card:hover {
    border-color: var(--app-fg, var(--accent, #8ab4f8));
    background: var(--surface-hover, #24243a);
}

.app-card:active {
    transform: translateY(1px);
}

.app-card:focus-visible {
    outline: 2px solid var(--app-fg, var(--accent, #8ab4f8));
    outline-offset: 2px;
}

.spawn-btn {
    display: block;
    width: 100%;
    margin-bottom: 4px;
    padding: 4px 8px;
    background: var(--surface, #2a2a4e);
    color: var(--text-muted, #c0c0c0);
    border: 1px solid var(--border-strong, #444);
    border-radius: 3px;
    cursor: pointer;
    font-size: 12px;
    text-align: left;
}

.spawn-btn:hover {
    background: var(--surface-hover, #3a3a5e);
}

.active-entry {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 3px 4px;
    font-size: 12px;
    border-radius: 3px;
}

.active-entry:hover {
    background: var(--surface, #2a2a4e);
}

.active-entry span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
}

.active-title {
    cursor: pointer;
}

.active-title:hover {
    color: var(--accent, #90d0ff);
}

.close-small {
    background: none;
    border: 1px solid var(--border-strong, #444);
    border-radius: 3px;
    color: var(--text-dim, #888);
    cursor: pointer;
    padding: 0 4px;
    font-size: 11px;
    margin-left: 4px;
    flex-shrink: 0;
}

.close-small:hover {
    background: var(--surface-hover, #3a3a5e);
    color: var(--text, #e0e0e0);
}

.window-area {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px;
    overflow-y: auto;
    min-height: 0; /* allow flex child to shrink and scroll */
}

/* Individual window sections */
.window {
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    display: flex;
    flex-direction: column;
    min-height: 200px;
    flex-shrink: 0;
}

/* Maximized window surface (reframe §4-B). One-deep: at most one window
   carries `.maximized` at a time. `position: fixed` promotes it to the WHOLE
   viewport — covering the status bar too, the same full-screen surface the
   site overlay gets — rather than growing inside the bordered window panel.
   (fixed escapes #dom-layer's absolute/overflow nesting: none of its
   ancestors establish a containing block for fixed positioning.) The window
   renders via its normal path; only its framing changes. Minimize removes
   the class. */
.window.maximized {
    position: fixed;
    inset: 0;
    z-index: 9999;
    min-height: 0;
    border-radius: 0;
    background: var(--surface-max, #14141c);
}

.window header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px;
    background: var(--surface-header, #1e1e3e);
    border-bottom: 1px solid var(--border, #333);
    border-radius: 4px 4px 0 0;
}

.window header h3 {
    margin: 0;
    font-size: 13px;
    color: var(--title-muted, #a0a0c0);
}

/* Window controls (maximize / restore / close) share one uniform, centered
   box so the differently-shaped glyphs (▢ ❐ ×) read as the same-size buttons —
   the bare "×" otherwise looked smaller — and give a comfortable tap target. */
.window header .close,
.window header .winctl {
    background: none;
    border: 1px solid var(--border-strong, #444);
    border-radius: 3px;
    color: var(--text-dim, #888);
    cursor: pointer;
    width: 26px;
    height: 22px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 14px;
    line-height: 1;
}

.window header .close:hover,
.window header .winctl:hover {
    background: var(--surface-hover, #3a3a5e);
    color: var(--text, #e0e0e0);
}

/* Header right-side control cluster so title stays left, buttons group right.
   `gap` spreads maximize and close apart so they're not easy to mis-tap. */
.window header .winctls {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: auto;
}

.window-content {
    display: flex;
    flex: 1;
    overflow: auto;
    min-height: 0;
}

/* Every window renders exactly one root wrapper into `.window-content`.
   `.window-content` is a flex row, so `align-items: stretch` already fills
   the child vertically — but with no horizontal grow the child collapses to
   its content width and "sits there" on a wide screen. Grow the single root
   so each window fills the whole panel horizontally (the behaviour Entity
   Tree already got from its explicit `width: 100%`). `min-width: 0` lets
   wide content (monospace scrollback, long rows) scroll instead of forcing
   the panel wider than the window. */
.window-content > * {
    flex: 1 1 auto;
    min-width: 0;
}

.entity-browser {
    display: flex;
    width: 100%;
    height: 100%;
}

/* Tree panel — left sidebar */
.tree-panel {
    width: 220px;
    min-width: 150px;
    overflow-y: auto;
    border-right: 1px solid var(--border, #333);
    padding: 8px;
    flex-shrink: 0;
}

.tree-panel h2 {
    margin: 0 0 8px 0;
    font-size: 16px;
}

.tree-panel footer {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--border, #333);
    font-size: 12px;
    color: var(--text-dim, #888);
}

.selection-source {
    display: block;
    width: 100%;
    margin-bottom: 8px;
    padding: 3px;
    /* A form control like every other <select> — use the shared input
       surface so it themes (light/dark) and matches the standard selects.
       Was a one-off `#1e1e1e` that read dark-on-dark in light mode and
       sat off-palette in dark mode. */
    background: var(--input-bg, #0e0e1e);
    color: var(--text, #e0e0e0);
    border: 1px solid var(--border-strong, #444);
    border-radius: 3px;
    font-size: 12px;
}

.selection-source-label {
    display: block;
    margin-bottom: 2px;
    font-size: 11px;
    color: var(--text-dim, #888);
}

.nav-up {
    display: block;
    margin-bottom: 8px;
    padding: 4px 8px;
    background: var(--surface, #2a2a4e);
    color: var(--text-muted, #c0c0c0);
    border: 1px solid var(--border-strong, #444);
    border-radius: 3px;
    cursor: pointer;
    font-size: 13px;
}

.nav-up:hover {
    background: var(--surface-hover, #3a3a5e);
}

/* Tree rows — flat list, indentation via inline padding-left. */
.tree-row {
    padding: 3px 8px;
    cursor: pointer;
    border-radius: 3px;
    white-space: nowrap;
    display: block;
}

.tree-row:hover {
    background: var(--surface, #2a2a4e);
}

.tree-row[aria-selected="true"] {
    background: var(--selected-bg, #2a4a6e);
    color: var(--accent, #90d0ff);
    font-weight: bold;
}

.tree-toggle {
    color: var(--title-muted, #a0a0c0);
    cursor: pointer;
    user-select: none;
}

.tree-toggle-spacer {
    display: inline-block;
    width: 1.4em;
}

.tree-leaf-count {
    color: var(--text-dim, #888);
    font-size: 0.9em;
}

/* Document panel — center */
.document-panel {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    min-width: 0;
}

.document-panel h1 {
    margin: 0 0 4px 0;
    font-size: 18px;
}

.entity-type {
    color: var(--text-dim, #888);
    margin: 0 0 12px 0;
}

.entity-content {
    white-space: pre-wrap;
    font-family: var(--font-mono, monospace);
    font-size: 13px;
    line-height: 1.5;
    background: var(--input-bg, #0e0e1e);
    padding: 12px;
    border-radius: 4px;
    overflow-x: auto;
}

.placeholder {
    color: var(--text-faint, #666);
    font-style: italic;
    text-align: center;
    padding-top: 40px;
}

/* Inspector panel — right sidebar */
.inspector-panel {
    width: 250px;
    min-width: 180px;
    overflow-y: auto;
    border-left: 1px solid var(--border, #333);
    padding: 8px;
    flex-shrink: 0;
}

.inspector-panel h2 {
    margin: 0 0 8px 0;
    font-size: 16px;
}

.inspector-panel dl {
    margin: 0;
}

.inspector-panel dt {
    font-weight: bold;
    color: var(--title-muted, #a0a0c0);
    margin-top: 6px;
}

.inspector-panel dd {
    margin: 2px 0 0 0;
}

.inspector-panel code {
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    background: var(--input-bg, #0e0e1e);
    padding: 1px 4px;
    border-radius: 2px;
}

.raw-hash {
    font-family: var(--font-mono, monospace);
    font-size: 11px;
    word-break: break-all;
    background: var(--input-bg, #0e0e1e);
    padding: 8px;
    border-radius: 4px;
}

/* Scanner section */
.scan-section {
    padding: 8px 12px;
    border-bottom: 1px solid var(--border, #333);
}

.scan-preview {
    margin-top: 8px;
}

.scan-status {
    color: var(--text-dim, #888);
    font-size: 12px;
    margin: 4px 0;
}

/* Peer Management window */
.peer-mgmt {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 12px;
}

.peer-mgmt-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
}

.peer-mgmt-header h2 {
    margin: 0;
}

.peer-create-panel {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
}

/* Deliberately NOT theme::INPUT (block/width:100%): inside the button
   flex row that collapses to a sliver. Keep a usable min width; the
   input wraps to its own line when the panel runs out of space. */
.peer-create-alias {
    flex: 1 1 160px;
    min-width: 140px;
    background: var(--input-bg, #0e0e1e);
    color: var(--text, #e0e0e0);
    border: 1px solid var(--border-strong, #444);
    padding: 4px 8px;
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    border-radius: 3px;
    box-sizing: border-box;
}

.peer-table-wrap {
    overflow-x: auto;
}

.peer-table {
    width: 100%;
    border-collapse: collapse;
}

.peer-table th {
    text-align: left;
    padding: 8px 10px;
    border-bottom: 2px solid var(--border-bold, #555);
    color: #999;
    font-size: 0.8em;
    text-transform: uppercase;
    letter-spacing: 0.05em;
}

.peer-table td {
    padding: 8px 10px;
    border-bottom: 1px solid var(--border, #333);
    vertical-align: middle;
}

.peer-table td.id {
    font-family: var(--font-mono, monospace);
    white-space: nowrap;
}

.peer-table td.actions {
    white-space: nowrap;
}

.peer-table td.addr-stopped {
    font-size: 0.85em;
    color: #886;
}

.peer-table td.addr-list,
.peer-table td.addr-none {
    font-family: var(--font-mono, monospace);
    font-size: 0.85em;
}

.peer-table td.addr-list {
    color: #aaa;
}

/* Per-row badge — color comes from kind modifier. */
.peer-badge {
    font-size: 0.75em;
    border: 1px solid currentColor;
    border-radius: 3px;
    padding: 1px 6px;
}

.peer-badge.primary { color: var(--peer-primary, #6b8); }
.peer-badge.local   { color: var(--peer-local, #8ab); }
.peer-badge.remote  { color: var(--peer-remote, #b8a); }

.peer-saved {
    font-size: 0.7em;
    color: var(--text-faint, #666);
    margin-left: 4px;
}

.peer-action-delete {
    margin-left: 6px;
}

/* Peer Connections — backend peer rows */
.peer-conn-backend-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
}

.peer-conn-backend-info {
    font-family: var(--font-mono, monospace);
    font-size: 0.85em;
    color: #aaa;
}

/* ---- Responsive: narrow screens / portrait mobile ---- */

@media (max-width: 768px) {
    /* Peers: stack heading over a full-width, wrapping create panel
       so the alias input is never squeezed out. */
    .peer-mgmt-header {
        flex-direction: column;
        align-items: stretch;
    }

    .peer-create-panel {
        width: 100%;
    }

    .peer-create-alias {
        flex: 1 1 100%;
        min-width: 0;
    }

    .peer-create-panel button {
        flex: 1 1 auto;
    }

    .window-manager {
        flex-direction: column;
    }

    /* The palette collapses behind a single `☰ Menu` toggle (see .palette-shell
       below). Closed by default it's just the toggle bar, so it no longer hogs
       the screen; when opened, only the BODY scrolls (capped), and .window-area
       (flex:1) keeps the rest. This fixes both the menu-eats-the-screen and the
       can't-scroll-on-mobile regressions from the menu redesign. */
    .command-palette {
        width: auto;
        border-right: none;
        border-bottom: 1px solid var(--border, #333);
        padding: 6px 8px;
        display: block;
        overflow-y: visible;
        max-height: none;
    }

    /* Split toggle bar: ☰ Menu (≈75%) | Open Windows (≈25%). Each gates its own
       panel so the active-windows list is one tap away without opening the menu. */
    .palette-bar {
        display: flex;
        gap: 6px;
    }

    .palette-toggle,
    .palette-windows-toggle {
        display: block;
        text-align: center;
        font-size: 16px; /* >=16px avoids iOS zoom-on-focus */
        font-weight: bold;
        padding: 12px 8px;
        background: transparent;
        color: var(--accent, #90d0ff);
        border: 1px solid var(--border, #333);
        border-radius: 4px;
        cursor: pointer;
        white-space: nowrap;
    }

    .palette-toggle {
        flex: 3;
        text-align: left;
    }

    .palette-windows-toggle {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    /* No windows → the toggle is disabled; dim it so the no-op is obvious. */
    .palette-windows-toggle:disabled {
        opacity: 0.4;
        cursor: default;
    }

    /* Both panels collapse until their respective shell class is set. */
    .palette-body,
    .palette-windows {
        display: none;
    }

    .palette-shell.menu-open > .palette-body,
    .palette-shell.windows-open > .palette-windows {
        display: block;
        max-height: 65vh;
        overflow-y: auto;
        margin-top: 6px;
    }

    /* Per-panel header bar — separates the menu from the open-windows list when
       both are open, and labels a lone-open windows panel so it's never
       ambiguous. A top accent border makes the section break obvious. */
    .palette-panel-head {
        display: block;
        font-size: 11px;
        font-weight: 700;
        letter-spacing: 0.06em;
        text-transform: uppercase;
        color: var(--text-dim, #888);
        padding: 7px 6px 5px;
        margin-bottom: 4px;
        border-top: 2px solid var(--accent, #90d0ff);
        border-bottom: 1px solid var(--border, #333);
    }

    /* On mobile the "Open Windows" toggle + panel head already label it — hide
       the inner details summary so it isn't shown twice. */
    .palette-windows > .palette-group > summary {
        display: none;
    }

    .command-palette h3 {
        margin: 4px 0 2px 0;
        font-size: 13px;
    }

    .palette-group > summary {
        font-size: 15px;
        padding: 10px 6px;
    }

    /* Touch targets: full-width rows, ~44px tall. */
    .spawn-btn {
        display: block;
        width: 100%;
        margin: 0 0 6px 0;
        font-size: 15px;
        padding: 11px 12px;
    }

    .active-entry {
        font-size: 14px;
        padding: 8px 6px;
    }

    .close-small {
        font-size: 15px;
        padding: 6px 12px;
    }

    .command-palette select {
        font-size: 16px; /* >=16px avoids iOS zoom-on-focus */
        padding: 10px;
    }

    .window-area {
        flex: 1;
    }

    /* Entity Browser: stack panels vertically */
    .window-content {
        flex-direction: column;
    }

    .tree-panel {
        width: auto;
        min-width: auto;
        border-right: none;
        border-bottom: 1px solid var(--border, #333);
        max-height: 200px;
        overflow-y: auto;
    }

    .inspector-panel {
        width: auto;
        min-width: auto;
        border-left: none;
        border-top: 1px solid var(--border, #333);
        max-height: 200px;
        overflow-y: auto;
    }

    .document-panel {
        flex: 1;
        min-height: 150px;
    }
}

/* Landscape on mobile — tree+inspector side by side, document below */
@media (max-width: 768px) and (orientation: landscape) {
    .window-content {
        flex-direction: row;
        flex-wrap: wrap;
    }

    .tree-panel {
        width: 40%;
        max-height: 150px;
        border-bottom: 1px solid var(--border, #333);
        border-right: 1px solid var(--border, #333);
    }

    .inspector-panel {
        width: 58%;
        max-height: 150px;
        border-top: none;
        border-bottom: 1px solid var(--border, #333);
    }

    .document-panel {
        width: 100%;
        flex-basis: 100%;
    }
}
"#;

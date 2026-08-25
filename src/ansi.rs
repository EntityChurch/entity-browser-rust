//! Minimal ANSI SGR → HTML converter, so colored `tracing` output (the backend
//! peer's logs) renders *with its colors* in the DOM instead of as raw escape
//! codes. Handles the sequences `tracing_subscriber`'s fmt writer emits: reset,
//! bold (1), dim (2), italic (3), underline (4), the 8 standard (30–37) and 8
//! bright (90–97) foreground colors, and their resets (22/23/24/39). Unknown
//! codes are ignored. All text is HTML-escaped, so the result is safe to assign
//! via `innerHTML`.
//!
//! Pure string→string (crate root, not the wasm-only `dom` module) so it is unit
//! tested on the native target.

/// Convert one line that may contain ANSI SGR escapes into HTML with
/// inline-styled `<span>`s. Text with no escapes is returned HTML-escaped as-is.
pub fn ansi_to_html(input: &str) -> String {
    let mut out = String::new();
    let mut style = Style::default();
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut text_start = 0;

    while i < bytes.len() {
        // An SGR sequence is ESC '[' … final-byte. ESC and '[' are ASCII, so the
        // byte indices land on char boundaries and the slices below are valid.
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            flush(&mut out, &input[text_start..i], &style);
            // Scan params (digits + ';') up to the final alphabetic byte.
            let mut j = i + 2;
            while j < bytes.len() && !bytes[j].is_ascii_alphabetic() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'm' {
                apply_sgr(&mut style, &input[i + 2..j]);
            }
            i = if j < bytes.len() { j + 1 } else { j };
            text_start = i;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &input[text_start..], &style);
    out
}

/// Accumulated SGR state. Default (all fields off) is the reset/plain state.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Option<&'static str>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

impl Style {
    /// The inline CSS for the current state, or `None` when default (so a plain
    /// run needs no wrapping span).
    fn css(&self) -> Option<String> {
        if *self == Style::default() {
            return None;
        }
        let mut css = String::new();
        if let Some(c) = self.fg {
            css.push_str("color:");
            css.push_str(c);
            css.push(';');
        }
        if self.bold {
            css.push_str("font-weight:600;");
        }
        if self.dim {
            css.push_str("opacity:0.7;");
        }
        if self.italic {
            css.push_str("font-style:italic;");
        }
        if self.underline {
            css.push_str("text-decoration:underline;");
        }
        Some(css)
    }
}

fn flush(out: &mut String, text: &str, style: &Style) {
    if text.is_empty() {
        return;
    }
    let escaped = escape_html(text);
    match style.css() {
        Some(css) => {
            out.push_str("<span style=\"");
            out.push_str(&css);
            out.push_str("\">");
            out.push_str(&escaped);
            out.push_str("</span>");
        }
        None => out.push_str(&escaped),
    }
}

/// Apply a `;`-separated SGR parameter list to `style`.
fn apply_sgr(style: &mut Style, params: &str) {
    // Bare `ESC[m` (empty) means reset, same as `0`.
    if params.is_empty() {
        *style = Style::default();
        return;
    }
    for part in params.split(';') {
        let code: u16 = match part.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        match code {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            30..=37 => style.fg = Some(color(code - 30, false)),
            39 => style.fg = None,
            90..=97 => style.fg = Some(color(code - 90, true)),
            _ => {} // ignore backgrounds, 256/truecolor, etc.
        }
    }
}

/// Map an ANSI color index (0..=7) to a CSS color legible on the log pane's dark
/// background. `bright` picks the lighter variant. Reuses the theme status tokens
/// where the mapping is natural (tracing colors levels: err=red, warn=yellow,
/// info=green, debug=blue).
fn color(idx: u16, bright: bool) -> &'static str {
    match (idx, bright) {
        (0, false) => "#888",                        // black → gray (visible on dark)
        (0, true) => "#aaa",
        (1, _) => "var(--status-err, #f66)",         // red
        (2, _) => "var(--status-ok, #6c6)",          // green
        (3, _) => "var(--status-warn, #fc9)",        // yellow
        (4, _) => "var(--status-info, #9cf)",        // blue
        (5, _) => "#c9f",                            // magenta
        (6, _) => "#9ff",                            // cyan
        (7, false) => "#ddd",                        // white
        (7, true) => "#fff",
        _ => "inherit",
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_escaped_unchanged() {
        assert_eq!(ansi_to_html("hello <world>"), "hello &lt;world&gt;");
    }

    #[test]
    fn colored_run_wraps_in_a_span() {
        // green "INFO" then reset
        let html = ansi_to_html("\x1b[32mINFO\x1b[0m rest");
        assert_eq!(
            html,
            "<span style=\"color:var(--status-ok, #6c6);\">INFO</span> rest"
        );
    }

    #[test]
    fn dim_and_italic_compose() {
        let html = ansi_to_html("\x1b[2m\x1b[3mfield\x1b[0m");
        assert_eq!(
            html,
            "<span style=\"opacity:0.7;font-style:italic;\">field</span>"
        );
    }

    #[test]
    fn bare_reset_clears_style() {
        // dim on, then bare ESC[m resets → trailing text is unstyled
        let html = ansi_to_html("\x1b[2mdim\x1b[mplain");
        assert_eq!(html, "<span style=\"opacity:0.7;\">dim</span>plain");
    }

    #[test]
    fn unknown_codes_are_ignored_text_survives() {
        // 48;5;12 (a background 256-color) is ignored; text still emitted.
        let html = ansi_to_html("\x1b[48;5;12mx\x1b[0m");
        assert_eq!(html, "x");
    }

    #[test]
    fn a_realistic_tracing_line_round_trips_text() {
        // dim timestamp, green INFO, dim target — assert the visible text is
        // preserved (order + content) regardless of the span wrapping.
        let line = "\x1b[2m2026-07-10T14:15:41Z\x1b[0m \x1b[32m INFO\x1b[0m \
                    \x1b[2mentity_peer::server\x1b[0m\x1b[2m:\x1b[0m accepted connection";
        let html = ansi_to_html(line);
        assert!(html.contains("2026-07-10T14:15:41Z"));
        assert!(html.contains("INFO"));
        assert!(html.contains("entity_peer::server"));
        assert!(html.contains("accepted connection"));
        // No raw escape bytes survive.
        assert!(!html.contains('\x1b'));
    }
}

//! Text-mode graphics for the System Monitor: rings of samples, braille
//! sparklines and block meters. Pure — samples in, strings out — so every
//! glyph the window draws is a native test away.
//!
//! **Why braille** (`DESIGN-2026-09-14-c` §3): U+2800–U+28FF packs a 2×4 dot
//! grid into one character cell, the highest resolution text can carry, and a
//! graph that is text is themed by CSS colour, needs no drawing library, and
//! reads as the terminal monitor the operator described. A screen reader reads
//! these cells as braille, so the renderer marks every graph `aria-hidden` and
//! puts the number beside it in words.

use std::collections::VecDeque;

/// How many samples a ring keeps: two minutes at 1 Hz.
pub const RING_SAMPLES: usize = 120;

/// A fixed-length history of one signal, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ring {
    samples: VecDeque<f64>,
}

impl Ring {
    pub fn push(&mut self, v: f64) {
        if self.samples.len() == RING_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(v);
    }

    pub fn last(&self) -> Option<f64> {
        self.samples.back().copied()
    }

    pub fn max(&self) -> f64 {
        self.samples.iter().copied().fold(0.0, f64::max)
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.samples.len()
    }

    /// The newest `n` samples, oldest first.
    pub fn tail(&self, n: usize) -> Vec<f64> {
        let skip = self.samples.len().saturating_sub(n);
        self.samples.iter().skip(skip).copied().collect()
    }
}

/// Dot bits of one braille cell, per column, bottom row first. The Unicode
/// braille dot numbering is 1-2-3-7 down the left column and 4-5-6-8 down the
/// right, which is why the bit order is not simply row order.
const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];

/// How many of `dots` stacked dot rows `v` fills against `scale`. Any non-zero
/// sample lights at least one dot: a graph that shows a flat line for "a
/// little" is a graph that says "nothing", which is a different fact.
fn level(v: f64, scale: f64, dots: usize) -> usize {
    // NaN compares false both ways, so it is ruled out explicitly.
    if v.is_nan() || scale.is_nan() || v <= 0.0 || scale <= 0.0 {
        return 0;
    }
    ((v / scale * dots as f64).ceil() as usize).clamp(1, dots)
}

/// A filled braille area graph of `samples` (oldest first), `cells` characters
/// wide, two samples per cell, scaled so `scale` fills the cell. The newest
/// samples sit at the right edge; missing history is blank on the left.
#[cfg(test)]
pub fn braille(samples: &[f64], scale: f64, cells: usize) -> String {
    braille_rows(samples, scale, cells, 1)
}

/// [`braille_rows`] with a **baseline**: a cell whose samples are zero draws its
/// bottom dots instead of nothing, so a quiet graph is a flat line and not a
/// blank box that reads as broken (screenshot, 2026-09-14). History that does
/// not exist yet stays blank — a line there would claim a measurement.
pub fn braille_axis(samples: &[f64], scale: f64, cells: usize, rows: usize) -> String {
    let rows = rows.max(1);
    let plain = braille_rows(samples, scale, cells, rows);
    let pad = (cells * 2).saturating_sub(samples.len());
    let mut lines: Vec<String> = plain.split('\n').map(String::from).collect();
    if let Some(bottom) = lines.last_mut() {
        *bottom = bottom
            .chars()
            .enumerate()
            .map(|(c, ch)| {
                let mut bits = ch as u32 - 0x2800;
                if c * 2 >= pad {
                    bits |= LEFT[0];
                }
                if c * 2 + 1 >= pad {
                    bits |= RIGHT[0];
                }
                char::from_u32(0x2800 + bits).expect("U+2800..U+28FF are all characters")
            })
            .collect();
    }
    lines.join("\n")
}

/// [`braille`], `rows` lines tall (joined by `\n`, top line first) — four dot
/// levels per row, so a three-row graph has twelve steps of height.
pub fn braille_rows(samples: &[f64], scale: f64, cells: usize, rows: usize) -> String {
    let rows = rows.max(1);
    let want = cells * 2;
    let tail = &samples[samples.len().saturating_sub(want)..];
    let pad = want - tail.len();
    let at = |i: usize| if i < pad { 0.0 } else { tail[i - pad] };
    // Each sample's total height in dots, across every row.
    let heights: Vec<usize> = (0..want).map(|i| level(at(i), scale, rows * 4)).collect();
    (0..rows)
        .rev()
        .map(|row| {
            (0..cells)
                .map(|c| {
                    let in_row = |h: usize| h.saturating_sub(row * 4).min(4);
                    let (l, r) = (in_row(heights[c * 2]), in_row(heights[c * 2 + 1]));
                    let bits = LEFT[..l].iter().sum::<u32>() + RIGHT[..r].iter().sum::<u32>();
                    char::from_u32(0x2800 + bits).expect("U+2800..U+28FF are all characters")
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A block meter `cells` wide: `█` for the filled fraction, `░` for the rest,
/// with one partial block at the boundary so a small change still moves it.
pub fn meter(fraction: f64, cells: usize) -> String {
    const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let eighths = (f * cells as f64 * 8.0).round() as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut s = "█".repeat(full.min(cells));
    if full < cells {
        if part > 0 {
            s.push(PARTIAL[part]);
            s.push_str(&"░".repeat(cells - full - 1));
        } else {
            s.push_str(&"░".repeat(cells - full));
        }
    }
    s
}

/// Bytes as a short human figure (B / KB / MB / GB, binary units).
pub fn bytes(n: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n.max(0.0);
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{v:.0} {}", UNITS[u]) // i18n-ignore — units are language-neutral
    } else {
        format!("{v:.1} {}", UNITS[u]) // i18n-ignore — units are language-neutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_scale_sample_fills_its_column_and_zero_is_the_blank_cell() {
        assert_eq!(braille(&[4.0, 4.0], 4.0, 1), "⣿");
        assert_eq!(braille(&[0.0, 0.0], 4.0, 1), "\u{2800}");
        // left column full, right empty: dots 1,2,3,7
        assert_eq!(braille(&[4.0, 0.0], 4.0, 1), "⡇");
        // right column full, left empty: dots 4,5,6,8
        assert_eq!(braille(&[0.0, 4.0], 4.0, 1), "⢸");
    }

    /// Levels fill from the BOTTOM — a graph whose bars hang from the top reads
    /// upside down, and the dot numbering makes that an easy mistake.
    #[test]
    fn a_quarter_lights_the_bottom_dot_of_its_column() {
        assert_eq!(braille(&[1.0, 0.0], 4.0, 1), "⡀"); // dot 7
        assert_eq!(braille(&[0.0, 1.0], 4.0, 1), "⢀"); // dot 8
        assert_eq!(braille(&[2.0, 0.0], 4.0, 1), "⡄"); // dots 3,7
    }

    #[test]
    fn a_little_is_not_nothing_and_over_scale_clamps() {
        assert_eq!(braille(&[0.001, 0.0], 100.0, 1), "⡀");
        assert_eq!(braille(&[1e9, 1e9], 1.0, 1), "⣿");
        assert_eq!(braille(&[f64::NAN, -3.0], 1.0, 1), "\u{2800}");
    }

    #[test]
    fn newest_samples_sit_at_the_right_and_short_history_pads_left() {
        let g = braille(&[4.0, 4.0], 4.0, 3);
        assert_eq!(g.chars().count(), 3);
        assert_eq!(g, "\u{2800}\u{2800}⣿");
        // more history than fits: only the newest 2*cells are drawn
        let g = braille(&[4.0, 4.0, 0.0, 0.0], 4.0, 1);
        assert_eq!(g, "\u{2800}");
    }

    /// A taller graph stacks: a full sample fills every row, a half fills the
    /// bottom row and leaves the top blank — bars grow UP, across rows too.
    #[test]
    fn a_multi_row_graph_fills_from_the_bottom_row_up() {
        assert_eq!(braille_rows(&[4.0, 4.0], 4.0, 1, 2), "⣿\n⣿");
        assert_eq!(braille_rows(&[2.0, 2.0], 4.0, 1, 2), "\u{2800}\n⣿");
        assert_eq!(braille_rows(&[3.0, 0.0], 4.0, 1, 2), "⡄\n⡇", "six of eight dots: bottom row full, two on top");
        for rows in 1..5 {
            assert_eq!(braille_rows(&[1.0; 9], 1.0, 7, rows).lines().count(), rows);
            assert!(braille_rows(&[1.0; 9], 1.0, 7, rows).lines().all(|l| l.chars().count() == 7));
        }
    }

    #[test]
    fn a_baseline_draws_zero_as_a_line_but_not_history_that_does_not_exist() {
        // two cells, three samples: the first half-cell has no history
        assert_eq!(braille_axis(&[0.0, 0.0, 0.0], 4.0, 2, 1), "⢀⣀");
        // a full sample is unchanged by the baseline
        assert_eq!(braille_axis(&[4.0, 4.0], 4.0, 1, 2), "⣿\n⣿");
        // only the bottom row carries it
        assert_eq!(braille_axis(&[0.0, 0.0], 4.0, 1, 2), "\u{2800}\n⣀");
    }

    #[test]
    fn a_meter_is_always_its_width_and_moves_by_eighths() {
        for f in [0.0, 0.01, 0.5, 0.99, 1.0, 7.0, f64::NAN] {
            assert_eq!(meter(f, 10).chars().count(), 10, "fraction {f}");
        }
        assert_eq!(meter(0.0, 4), "░░░░");
        assert_eq!(meter(1.0, 4), "████");
        assert_eq!(meter(0.5, 4), "██░░");
        assert_eq!(meter(0.125, 1), "▏"); // one eighth of one cell
    }

    #[test]
    fn a_ring_keeps_two_minutes_and_tails_oldest_first() {
        let mut r = Ring::default();
        for i in 0..(RING_SAMPLES + 5) {
            r.push(i as f64);
        }
        assert_eq!(r.count(), RING_SAMPLES);
        assert_eq!(r.last(), Some((RING_SAMPLES + 4) as f64));
        assert_eq!(r.tail(3), vec![(RING_SAMPLES + 2) as f64, (RING_SAMPLES + 3) as f64, (RING_SAMPLES + 4) as f64]);
        assert_eq!(r.max(), (RING_SAMPLES + 4) as f64);
    }

    #[test]
    fn bytes_read_in_binary_units() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KB");
        assert_eq!(bytes(256.0 * 1024.0 * 1024.0), "256.0 MB");
    }
}

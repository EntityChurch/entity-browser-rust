//! System Monitor DOM renderer — pure consumer of
//! [`MonitorOutput`](crate::views::system_monitor::output::MonitorOutput).
//!
//! Text mode (`DESIGN-2026-09-14-c` §3): bordered panes, braille graphs, block
//! meters, all from theme tokens. Each graph is `aria-hidden` with its figure
//! beside it in words, because a screen reader reads a braille cell as braille.
//! Each figure says where it came from.
//!
//! **Every figure also says what it means.** The first version showed accurate
//! numbers a person could not interpret — *longest pause*, *stalls*, a meter
//! against WebAssembly's 4 GB ceiling — and called two VMs that were freezing
//! the tab *idle*, because their work is not drawing (field report 2026-09-14).
//! So the tab pane leads with how it feels, frozen time is split into the part
//! this app explains and the part it does not, and an app window says *not
//! reporting* rather than *idle* when the app has not told us.

use crate::action::Action;
use crate::dom::components::{self, ButtonKind};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::monitor::graph::{braille_axis, bytes, meter};
use crate::monitor::sampler::WindowHistory;
use crate::monitor::{app_load, feel, load, Feel, Load};
use crate::views::storage::output::EstimateUnavailable;
use crate::views::system_monitor::output::{other_work, Engine, MonitorOutput, OtherWork, Source};
use crate::views::system_monitor::ABOUT_EVENT;

use web_sys::Element;

/// Cells in a pane-wide graph: 60 cells, two samples each = the full two minutes.
const GRAPH_CELLS: usize = 60;
/// Rows (lines) in the tab pane's frozen-time graph.
const FROZEN_ROWS: usize = 3;
const DRAWING_ROWS: usize = 1;
/// Cells in a window row's graph: the last 40 seconds.
const ROW_GRAPH_CELLS: usize = 20;
/// Cells in a meter.
const METER_CELLS: usize = 16;

fn t(key: &str, args: &[(&str, &str)]) -> String {
    crate::i18n::t(key, args)
}

pub fn render(container: &Element, out: &MonitorOutput, ctx: &DomCtx) {
    util::clear_children(container);
    let wrapper = util::create_element_with_class("div", "system-monitor");
    wrapper.set_attribute("style", theme::SECTION).ok();
    wrapper.set_attribute("data-field", "monitor-window").ok();

    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::TITLE_INLINE).ok();
    util::set_text(&h2, &t("window.system_monitor", &[]));
    util::append(&header, &h2);
    util::append(&wrapper, &header);
    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &t("monitor.hint", &[]));
    util::append(&wrapper, &hint);

    let grid = util::create_element("div");
    grid.set_attribute("style", theme::MONITOR_GRID).ok();
    util::append(&grid, &cpu_pane(out));
    util::append(&grid, &memory_pane(out));
    util::append(&grid, &storage_pane(out));
    util::append(&wrapper, &grid);

    util::append(&wrapper, &windows_pane(out, ctx));

    // The drawer sits in its own block with room above: a pane's title is set
    // into its top border, and straight under the button it overlapped the
    // button's text (field report 2026-09-14).
    let drawer = util::create_element("div");
    drawer.set_attribute("style", theme::MONITOR_DRAWER).ok();
    let about = components::button(
        ctx,
        &format!("{} {}", t("monitor.about", &[]), if out.about_open { "▾" } else { "▸" }), // i18n-ignore — disclosure arrow
        ButtonKind::Secondary,
        ABOUT_EVENT,
    );
    about.set_attribute("data-field", "monitor-about-toggle").ok();
    about.set_attribute("aria-expanded", if out.about_open { "true" } else { "false" }).ok();
    util::append(&drawer, &about);
    if out.about_open {
        util::append(&drawer, &about_pane(out));
    }
    util::append(&wrapper, &drawer);

    util::append(container, &wrapper);
}

fn pane(title: &str, field: &str) -> Element {
    let p = util::create_element("div");
    p.set_attribute("style", theme::MONITOR_PANE).ok();
    p.set_attribute("data-field", field).ok();
    let h = util::create_element("div");
    h.set_attribute("style", theme::MONITOR_PANE_TITLE).ok();
    util::set_text(&h, title);
    util::append(&p, &h);
    p
}

/// A meter, or a graph short enough never to overflow — starts at the left.
fn meter_el(text: &str, warn: bool) -> Element {
    let g = util::create_element("div");
    g.set_attribute("style", if warn { theme::MONITOR_METER_WARN } else { theme::MONITOR_METER }).ok();
    g.set_attribute("aria-hidden", "true").ok();
    util::set_text(&g, text);
    g
}

/// A pane-wide time series: newest at the right, and a pane narrower than the
/// graph loses the OLDEST samples (see `theme::MONITOR_GRAPH`).
fn graph(text: &str, warn: bool) -> Element {
    let g = util::create_element("div");
    g.set_attribute("style", if warn { theme::MONITOR_GRAPH_WARN } else { theme::MONITOR_GRAPH }).ok();
    g.set_attribute("aria-hidden", "true").ok();
    let inner = util::create_element("span");
    inner.set_attribute("style", theme::MONITOR_GRAPH_TEXT).ok();
    util::set_text(&inner, text);
    util::append(&g, &inner);
    g
}

fn source_label(source: Source) -> Option<String> {
    let key = match source {
        Source::Browser => "monitor.src_browser",
        Source::Counted => "monitor.src_counted",
        Source::App => "monitor.src_app",
        Source::Unavailable => return None,
    };
    Some(t(key, &[]))
}

/// A figure line, with its source after it.
fn line(parent: &Element, text: &str, source: Source, field: Option<(&str, String)>) {
    line_styled(parent, text, source, field, theme::MONITOR_LINE);
}

fn line_styled(parent: &Element, text: &str, source: Source, field: Option<(&str, String)>, style: &str) {
    let l = util::create_element("div");
    l.set_attribute("style", style).ok();
    if let Some((name, value)) = field {
        l.set_attribute("data-field", name).ok();
        l.set_attribute("data-value", &value).ok();
    }
    util::set_text(&l, text);
    if let Some(label) = source_label(source) {
        let s = util::create_element("span");
        s.set_attribute("style", theme::MONITOR_SOURCE).ok();
        util::set_text(&s, &format!(" · {label}")); // i18n-ignore — separator before a localized label
        util::append(&l, &s);
    }
    util::append(parent, &l);
}

/// A plain-language note under a figure: what it means, not what it is.
fn note(parent: &Element, text: &str) {
    let n = util::create_element("div");
    n.set_attribute("style", theme::MONITOR_NOTE).ok();
    util::set_text(&n, text);
    util::append(parent, &n);
}

fn cpu_pane(out: &MonitorOutput) -> Element {
    let h = &out.history;
    let p = pane(&t("monitor.cpu", &[]), "monitor-cpu");

    let verdict = feel(&h.frozen.tail(crate::monitor::LOAD_WINDOW_S));
    let (key, style, word) = match verdict {
        Feel::Smooth => ("monitor.feel_smooth", theme::MONITOR_FEEL_OK, "smooth"),
        Feel::Sluggish => ("monitor.feel_sluggish", theme::MONITOR_FEEL_WARN, "sluggish"),
        Feel::Struggling => ("monitor.feel_struggling", theme::MONITOR_FEEL_BAD, "struggling"),
        Feel::Unknown => ("monitor.feel_unknown", theme::MONITOR_LINE, "unknown"),
    };
    line_styled(&p, &t(key, &[]), Source::Unavailable, Some(("monitor-feel", word.to_string())), style);

    // Frozen time is the graph a person can feel; scaled to at least a tenth of
    // a second, so a quiet tab draws a low line instead of every flicker
    // filling the box.
    let frozen = h.frozen.tail(GRAPH_CELLS * 2);
    let frozen_max = h.frozen.max();
    util::append(&p, &graph(&braille_axis(&frozen, frozen_max.max(100.0), GRAPH_CELLS, FROZEN_ROWS), verdict == Feel::Struggling));
    let frozen_ms = h.frozen.last().unwrap_or(0.0);
    let drawing_ms = h.frozen_drawing.last().unwrap_or(0.0);
    let other_ms = (frozen_ms - drawing_ms).max(0.0);
    line(
        &p,
        &t("monitor.frozen", &[("ms", &format!("{frozen_ms:.0}"))]),
        Source::Counted,
        Some(("monitor-frozen", format!("{frozen_ms:.1}"))),
    );
    line(&p, &t("monitor.frozen_drawing", &[("ms", &format!("{drawing_ms:.0}"))]), Source::Counted, None);
    line(&p, &t("monitor.frozen_other", &[("ms", &format!("{other_ms:.0}"))]), Source::Counted, Some(("monitor-frozen-other", format!("{other_ms:.1}"))));
    note(&p, &t("monitor.frozen_explain", &[]));

    let running: Vec<String> = h.windows.iter().filter_map(|w| w.app.clone()).collect();
    let engine = out.probes.facts.as_ref().map(|f| f.engine);
    // Judged on the last few seconds, not the last one, so the note does not
    // flicker in and out with every GC.
    let recent_other = {
        let f = h.frozen.tail(crate::monitor::LOAD_WINDOW_S);
        let d = h.frozen_drawing.tail(crate::monitor::LOAD_WINDOW_S);
        if f.is_empty() { 0.0 } else { f.iter().zip(d.iter()).map(|(f, d)| (f - d).max(0.0)).sum::<f64>() / f.len() as f64 }
    };
    match other_work(engine, recent_other, &running) {
        OtherWork::Quiet => {}
        OtherWork::FirefoxApps(apps) => note(&p, &t("monitor.other_firefox_apps", &[("apps", &apps.join(", "))])), // i18n-ignore — list separator
        OtherWork::Unexplained => note(&p, &t("monitor.other_unexplained", &[])),
    }

    // This app's own drawing, on its own small graph.
    let work = h.frame_work.tail(GRAPH_CELLS * 2);
    util::append(&p, &graph(&braille_axis(&work, h.frame_work.max().max(100.0), GRAPH_CELLS, DRAWING_ROWS), h.frame_work.max() > 500.0));
    let ms = h.frame_work.last().unwrap_or(0.0);
    let fps = h.fps.last().unwrap_or(0.0);
    line(
        &p,
        &t("monitor.frame_work", &[("ms", &format!("{ms:.0}")), ("fps", &format!("{fps:.0}"))]),
        Source::Counted,
        Some(("monitor-frame-work", format!("{ms:.1}"))),
    );
    line(
        &p,
        &t(
            "monitor.freezes",
            &[
                ("ms", &format!("{:.0}", h.longest_gap.last().unwrap_or(0.0))),
                ("n", &h.stalls_total.to_string()),
            ],
        ),
        Source::Counted,
        Some(("monitor-stalls", h.stalls_total.to_string())),
    );
    if let Some(facts) = &out.probes.facts {
        if facts.pressure_api {
            match &out.probes.pressure {
                Some(state) => line(&p, &t("monitor.pressure", &[("state", state)]), Source::Browser, None),
                None => line(&p, &t("monitor.pressure_waiting", &[]), Source::Browser, None),
            }
        }
    }
    p
}

fn memory_pane(out: &MonitorOutput) -> Element {
    let p = pane(&t("monitor.memory", &[]), "monitor-memory");
    match out.probes.wasm_memory {
        Some(b) => {
            line(&p, &format!("{} {}", t("monitor.wasm", &[]), bytes(b)), Source::Browser, Some(("monitor-wasm", format!("{b:.0}")))); // i18n-ignore — label + figure
            note(&p, &t("monitor.wasm_explain", &[]));
        }
        None => line(&p, &t("state.loading", &[]), Source::Unavailable, None),
    }
    let device = out.probes.facts.as_ref().and_then(|f| f.device_memory_gb);
    match out.probes.js_heap {
        Some(heap) => {
            util::append(&p, &meter_el(&meter(heap.used / heap.limit, METER_CELLS), heap.used / heap.limit > 0.8));
            line(
                &p,
                &t("monitor.js_heap_of", &[("used", &bytes(heap.used)), ("limit", &bytes(heap.limit))]),
                Source::Browser,
                None,
            );
        }
        None if device.is_none() => note(&p, &t("monitor.memory_hidden", &[])),
        None => note(&p, &t("monitor.not_available", &[("what", &t("monitor.js_heap", &[]))])),
    }
    if let Some(gb) = device {
        line(&p, &t("monitor.device_memory", &[("gb", &format!("{gb}"))]), Source::Browser, None);
    }

    // What apps say they hold — and that the rest are not counted anywhere.
    let mut silent = false;
    for w in &out.history.windows {
        let Some(app) = &w.app else { continue };
        match w.app_memory {
            Some(m) => line(&p, &t("monitor.app_memory", &[("app", app), ("bytes", &bytes(m))]), Source::App, None),
            None => silent = true,
        }
    }
    if silent {
        note(&p, &t("monitor.app_memory_silent", &[]));
    }
    p
}

fn storage_pane(out: &MonitorOutput) -> Element {
    let p = pane(&t("monitor.storage", &[]), "monitor-storage");
    match &out.probes.storage {
        Some(Ok(s)) => {
            p.set_attribute("data-state", "known").ok();
            util::append(&p, &meter_el(&meter(if s.quota > 0.0 { s.usage / s.quota } else { 0.0 }, METER_CELLS), false));
            line(
                &p,
                &t("monitor.storage_used", &[("used", &bytes(s.usage)), ("quota", &bytes(s.quota))]),
                Source::Browser,
                None,
            );
            match s.persisted {
                Some(true) => line(&p, &t("monitor.persisted_yes", &[]), Source::Browser, None),
                Some(false) => line(&p, &t("monitor.persisted_no", &[]), Source::Browser, None),
                None => {}
            }
        }
        Some(Err(why)) => {
            p.set_attribute(
                "data-state",
                match why {
                    EstimateUnavailable::InsecureContext => "insecure-context",
                    EstimateUnavailable::NoApi => "no-api",
                    EstimateUnavailable::Failed => "failed",
                },
            )
            .ok();
            note(&p, &why.explanation());
        }
        None => {
            p.set_attribute("data-state", "pending").ok();
            line(&p, &t("state.loading", &[]), Source::Unavailable, None);
        }
    }
    p
}

/// A row's figure for sorting: how much of the last few seconds it held,
/// counting what the app says it did as well as what drawing it cost.
fn weight(w: &WindowHistory) -> f64 {
    let n = crate::monitor::LOAD_WINDOW_S;
    let drawing = w.render.tail(n).iter().sum::<f64>();
    let app = if w.app_reporting() { w.app_busy.tail(n).iter().sum::<f64>() } else { 0.0 };
    drawing.max(app)
}

fn load_word(l: Load) -> (&'static str, &'static str) {
    match l {
        Load::Idle => ("monitor.load_idle", "idle"),
        Load::Active => ("monitor.load_active", "active"),
        Load::Busy => ("monitor.load_busy", "busy"),
        Load::Unknown => ("monitor.load_unknown", "unknown"),
    }
}

fn windows_pane(out: &MonitorOutput, ctx: &DomCtx) -> Element {
    let p = pane(&t("monitor.windows", &[]), "monitor-windows");
    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &t("monitor.windows_hint", &[]));
    util::append(&p, &hint);
    let (table, tbody) = components::table(&[&t("monitor.col_window", &[]), &t("monitor.col_drawing", &[]), &t("monitor.col_app", &[]), ""]);
    let mut rows = out.history.windows.clone();
    // Busiest first, so the window to close is at the top.
    rows.sort_by(|a, b| weight(b).partial_cmp(&weight(a)).unwrap_or(std::cmp::Ordering::Equal).then(a.id.cmp(&b.id)));
    for w in &rows {
        // ── which window: its title (Apps · the app it runs) and its number
        let name = util::create_element("div");
        util::set_text(&name, &w.title);
        let tag = util::create_element("div");
        tag.set_attribute("style", theme::MONITOR_SOURCE).ok();
        let mut tag_text = t("monitor.window_number", &[("n", &w.id.to_string())]);
        if w.id == out.own_window {
            tag_text = format!("{tag_text} · {}", t("monitor.this_monitor", &[])); // i18n-ignore — separator
        }
        util::set_text(&tag, &tag_text);
        util::append(&name, &tag);

        // What the window fetched for itself — B-6. Only a window that counts
        // its fetches can have a figure; the rest say nothing rather than zero.
        if w.fetched_bytes > 0.0 {
            let net = util::create_element("div");
            net.set_attribute("style", theme::MONITOR_SOURCE).ok();
            net.set_attribute("data-field", "monitor-window-fetched").ok();
            net.set_attribute("data-value", &format!("{:.0}", w.fetched_bytes)).ok();
            util::set_text(&net, &t("monitor.window_fetched", &[("bytes", &bytes(w.fetched_bytes))]));
            util::append(&name, &net);
        }


        // ── drawing: what this app spent redrawing the window
        let series = w.render.tail(ROW_GRAPH_CELLS * 2);
        let drawing_verdict = load(&w.render.tail(crate::monitor::LOAD_WINDOW_S));
        let drawing = util::create_element("div");
        util::append(&drawing, &meter_el(&braille_axis(&series, w.render.max().max(50.0), ROW_GRAPH_CELLS, 1), drawing_verdict == Load::Busy));
        let ms = w.render.last().unwrap_or(0.0);
        let figure = util::create_element("div");
        figure.set_attribute("style", theme::MONITOR_SOURCE).ok();
        let (dkey, _) = load_word(drawing_verdict);
        util::set_text(&figure, &format!("{} · {}", t("monitor.render_ms", &[("ms", &format!("{ms:.1}"))]), t(dkey, &[]))); // i18n-ignore — separator
        util::append(&drawing, &figure);

        // ── the app inside, if any: what it reported, or that it did not
        let app_cell = util::create_element("div");
        let app_state = match &w.app {
            None => {
                util::set_text(&app_cell, "—"); // i18n-ignore — no app in this window
                "none"
            }
            Some(_) if w.app_reporting() => {
                let verdict = app_load(&w.app_busy.tail(crate::monitor::LOAD_WINDOW_S));
                util::append(&app_cell, &meter_el(&braille_axis(&w.app_busy.tail(ROW_GRAPH_CELLS * 2), 1000.0, ROW_GRAPH_CELLS, 1), verdict == Load::Busy));
                let busy = w.app_busy.last().unwrap_or(0.0);
                let (akey, aword) = load_word(verdict);
                let fig = util::create_element("div");
                fig.set_attribute("style", theme::MONITOR_SOURCE).ok();
                let mut text = format!("{} · {}", t("monitor.app_core", &[("pct", &format!("{:.0}", busy / 10.0))]), t(akey, &[])); // i18n-ignore — separator
                if let Some(ips) = w.app_ips.last().filter(|v| *v > 0.0) {
                    text = format!("{text} · {}", t("monitor.app_ips", &[("m", &format!("{:.1}", ips / 1e6))])); // i18n-ignore — separator
                }
                util::set_text(&fig, &format!("{text} · {}", t("monitor.src_app", &[]))); // i18n-ignore — separator
                util::append(&app_cell, &fig);
                aword
            }
            Some(_) => {
                let fig = util::create_element("div");
                fig.set_attribute("style", theme::MONITOR_SOURCE).ok();
                util::set_text(&fig, &t(if w.app_quiet_s.is_some() { "monitor.app_stopped_reporting" } else { "monitor.app_not_reporting" }, &[]));
                util::append(&app_cell, &fig);
                "not-reporting"
            }
        };
        if w.data_bytes > 0.0 || w.downloaded_bytes > 0.0 {
            let data = util::create_element("div");
            data.set_attribute("style", theme::MONITOR_SOURCE).ok();
            data.set_attribute("data-field", "monitor-app-data").ok();
            data.set_attribute("data-value", &format!("{:.0}", w.data_bytes)).ok();
            util::set_text(
                &data,
                &t("monitor.app_data", &[("bytes", &bytes(w.data_bytes)), ("downloaded", &bytes(w.downloaded_bytes))]),
            );
            util::append(&app_cell, &data);
        }

        // ── Show and Close
        let actions = util::create_element("div");
        actions.set_attribute("style", theme::MONITOR_ROW_ACTIONS).ok();
        let show = components::button_el(&t("monitor.show", &[]), ButtonKind::Small);
        show.set_attribute("data-field", "monitor-show").ok();
        ctx.on_action(&show, "click", Action::ShowWindow(w.id));
        util::append(&actions, &show);
        let close = components::button_el(&t("monitor.close", &[]), ButtonKind::Small);
        close.set_attribute("data-field", "monitor-close").ok();
        ctx.on_action(&close, "click", Action::CloseWindow(w.id));
        util::append(&actions, &close);

        let row = components::tr(vec![components::td(&name), components::td(&drawing), components::td(&app_cell), components::td(&actions)]);
        row.set_attribute("data-field", "monitor-window-row").ok();
        row.set_attribute("data-window-type", w.type_name.as_str()).ok();
        row.set_attribute("data-window-id", &w.id.to_string()).ok();
        row.set_attribute("data-load", load_word(drawing_verdict).1).ok();
        row.set_attribute("data-app-load", app_state).ok();
        util::append(&tbody, &row);
    }
    util::append(&p, &table);
    p
}

fn about_pane(out: &MonitorOutput) -> Element {
    let p = pane(&t("monitor.about", &[]), "monitor-about");
    let Some(f) = &out.probes.facts else { return p };
    let engine = match f.engine {
        Engine::Chromium => "Chromium", // i18n-ignore — proper noun
        Engine::Firefox => "Firefox",   // i18n-ignore — proper noun
        Engine::WebKit => "WebKit",     // i18n-ignore — proper noun
        Engine::Unknown => "?",         // i18n-ignore — unknown marker
    };
    line(&p, &t("monitor.engine", &[("engine", engine)]), Source::Browser, None);
    if let Some(c) = f.cores {
        line(&p, &t("monitor.cores", &[("n", &format!("{c}"))]), Source::Browser, None);
    }
    line(
        &p,
        &t(if f.cross_origin_isolated { "monitor.isolated_yes" } else { "monitor.isolated_no" }, &[]),
        Source::Browser,
        None,
    );
    if let Some(res) = f.timer_resolution_ms {
        line(
            &p,
            &t("monitor.timers", &[("ms", &format!("{res}"))]),
            Source::Counted,
            Some(("monitor-timer-resolution", format!("{res}"))),
        );
    }
    line(
        &p,
        &t(
            match f.engine {
                Engine::Firefox => "monitor.model_firefox",
                Engine::Chromium => "monitor.model_chromium",
                _ => "monitor.model_other",
            },
            &[],
        ),
        Source::Unavailable,
        None,
    );
    line(&p, &t(if f.long_frames { "monitor.long_frames_yes" } else { "monitor.long_frames_no" }, &[]), Source::Unavailable, None);
    if !f.pressure_api {
        line(&p, &t("monitor.not_available", &[("what", &t("monitor.pressure_name", &[]))]), Source::Unavailable, None);
    }
    line(&p, &t("monitor.app_reports", &[]), Source::Unavailable, None);
    line(&p, &t("monitor.hidden", &[]), Source::Unavailable, None);
    p
}

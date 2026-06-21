//! QR code scanner — photo capture + live video, with result log.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::dom::util;
use crate::window::ClosureVec;

fn has_barcode_detector() -> bool {
    js_sys::Reflect::get(
        &wasm_bindgen::JsValue::from(web_sys::window().unwrap()),
        &"BarcodeDetector".into(),
    )
    .map(|v| !v.is_undefined())
    .unwrap_or(false)
}

fn make_detector() -> Option<wasm_bindgen::JsValue> {
    let class = js_sys::Reflect::get(
        &wasm_bindgen::JsValue::from(web_sys::window().unwrap()),
        &"BarcodeDetector".into(),
    ).ok()?;
    let formats = js_sys::Array::new();
    formats.push(&"qr_code".into());
    let opts = js_sys::Object::new();
    js_sys::Reflect::set(&opts, &"formats".into(), &formats).ok();
    js_sys::Reflect::construct(
        &class.dyn_into::<js_sys::Function>().ok()?,
        &js_sys::Array::of1(&opts),
    ).ok()
}

fn detect_from(detector: &wasm_bindgen::JsValue, source: &wasm_bindgen::JsValue) -> Option<js_sys::Promise> {
    let detect_fn: js_sys::Function = js_sys::Reflect::get(detector, &"detect".into())
        .ok()?
        .dyn_into()
        .ok()?;
    let result = detect_fn.call1(detector, source).ok()?;
    Some(js_sys::Promise::from(result))
}

/// Create scanner UI: photo capture, live scanner, and results log.
///
/// `on_scan` is invoked with each decoded code's raw text (the QR payload).
/// The Peer Connections window uses it to populate the connect-address input
/// so a scan doesn't have to be retyped by hand.
pub fn create_scanner(
    container: &web_sys::Element,
    on_scan: Rc<dyn Fn(String)>,
    _active: Rc<RefCell<bool>>,
    closures: &ClosureVec,
) {
    // Results log — shared by both scan methods.
    let log = util::create_element_with_class("div", "scan-log");
    let log_header = util::create_element("h4");
    log_header.set_attribute("style", "margin:8px 0 4px;font-size:13px;color:var(--text-dim, #888)").ok();
    util::set_text(&log_header, "Scanned Codes");
    let log_list = util::create_element("div");
    log_list.set_attribute("style", "font-family:monospace;font-size:12px").ok();

    let found_codes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    // ---- Photo capture section ----
    let photo_section = util::create_element("div");
    photo_section.set_attribute("style", "margin:8px 0;padding:8px 0;border-bottom:1px solid var(--border, #333)").ok();

    let photo_title = util::create_element("strong");
    util::set_text(&photo_title, "Photo Capture");
    util::append(&photo_section, &photo_title);

    let photo_status = util::create_element("p");
    photo_status.set_attribute("style", "color:var(--text-dim, #888);font-size:12px;margin:4px 0").ok();
    util::set_text(&photo_status, "Take a photo of a QR code");

    let photo_label = util::create_element("label");
    photo_label.set_attribute("style",
        "display:inline-block;margin:4px 0;padding:6px 12px;background:var(--surface, #2a2a4e);color:var(--text-muted, #c0c0c0);\
         border:1px solid var(--border-strong, #444);border-radius:3px;cursor:pointer;font-size:13px"
    ).ok();
    util::set_text(&photo_label, "Take Photo");

    let file_input = util::document().create_element("input").unwrap();
    file_input.set_attribute("type", "file").ok();
    file_input.set_attribute("accept", "image/*").ok();
    file_input.set_attribute("capture", "environment").ok();
    file_input.set_attribute("style", "display:none").ok();
    util::append(&photo_label, &file_input);

    // Photo preview (reused, not accumulated).
    let photo_preview = util::create_element("div");

    {
        let status_ref = photo_status.clone();
        let preview_ref = photo_preview.clone();
        let log_list_ref = log_list.clone();
        let found_ref = found_codes.clone();
        let on_scan_ref = on_scan.clone();
        let closures_for_nested = closures.clone();

        util::listen(&file_input, "change", move |event: web_sys::Event| {
            let input: web_sys::HtmlInputElement = event.target().unwrap().dyn_into().unwrap();
            let files = input.files().unwrap();
            if files.length() == 0 { return; }
            let file = files.get(0).unwrap();

            // Clear previous preview.
            util::clear_children(&preview_ref);
            util::set_text(&status_ref, "Processing...");

            let url = web_sys::Url::create_object_url_with_blob(&file).unwrap();
            let img = util::document().create_element("img").unwrap()
                .dyn_into::<web_sys::HtmlImageElement>().unwrap();
            img.set_src(&url);
            img.style().set_property("max-width", "250px").ok();
            img.style().set_property("border-radius", "4px").ok();
            img.style().set_property("display", "block").ok();
            img.style().set_property("margin", "4px 0").ok();
            preview_ref.append_child(&img.clone().into()).ok();

            if !has_barcode_detector() {
                util::set_text(&status_ref, "No BarcodeDetector — enter code manually");
                return;
            }

            let img_c = img.clone();
            let status_c = status_ref.clone();
            let log_c = log_list_ref.clone();
            let found_c = found_ref.clone();
            let on_scan_c = on_scan_ref.clone();

            let onload = Closure::wrap(Box::new(move || {
                let detector = match make_detector() {
                    Some(d) => d,
                    None => { util::set_text(&status_c, "Detector init failed"); return; }
                };
                let promise = match detect_from(&detector, &img_c) {
                    Some(p) => p,
                    None => { util::set_text(&status_c, "Detect call failed"); return; }
                };

                let status_cc = status_c.clone();
                let log_cc = log_c.clone();
                let found_cc = found_c.clone();
                let on_scan_cc = on_scan_c.clone();

                wasm_bindgen_futures::spawn_local(async move {
                    match JsFuture::from(promise).await {
                        Ok(barcodes) => {
                            let arr: js_sys::Array = match barcodes.dyn_into() {
                                Ok(a) => a, Err(_) => js_sys::Array::new(),
                            };
                            if arr.length() > 0 {
                                let first = arr.get(0);
                                if let Ok(raw) = js_sys::Reflect::get(&first, &"rawValue".into()) {
                                    if let Some(text) = raw.as_string() {
                                        add_to_log(&log_cc, &found_cc, &text);
                                        on_scan_cc(text.clone());
                                        util::set_text(&status_cc, "QR code found!");
                                        status_cc.set_attribute("style", "color:var(--status-ok, #0f0);font-size:12px;margin:4px 0").ok();
                                        return;
                                    }
                                }
                            }
                            util::set_text(&status_cc, "No QR found — try again");
                        }
                        Err(_) => { util::set_text(&status_cc, "Decode error"); }
                    }
                });
            }) as Box<dyn Fn()>);

            img.set_onload(Some(onload.as_ref().unchecked_ref()));
            closures_for_nested.borrow_mut().push(onload.into_js_value());

            // Reset input so same file can be selected again.
            input.set_value("");
        }, closures);
    }

    util::append(&photo_section, &photo_label);
    util::append(&photo_section, &photo_status);
    util::append(&photo_section, &photo_preview);
    util::append(container, &photo_section);

    // ---- Live scanner section ----
    if has_barcode_detector() {
        let live_section = util::create_element("div");
        live_section.set_attribute("style", "margin:8px 0;padding:8px 0;border-bottom:1px solid var(--border, #333)").ok();

        let live_title = util::create_element("strong");
        util::set_text(&live_title, "Live Scanner");
        util::append(&live_section, &live_title);

        let live_status = util::create_element("p");
        live_status.set_attribute("style", "color:var(--text-dim, #888);font-size:12px;margin:4px 0").ok();
        util::set_text(&live_status, "Real-time camera scanning");

        let live_preview = util::create_element("div");
        let live_active = Rc::new(RefCell::new(false));

        let start_btn = util::create_element_with_class("button", "spawn-btn");
        util::set_text(&start_btn, "Start Live Scan");

        let stop_btn = util::create_element_with_class("button", "spawn-btn");
        util::set_text(&stop_btn, "Stop");
        stop_btn.set_attribute("style",
            "display:none;margin-left:4px;padding:4px 10px;background:#4a2a2a;color:var(--text-muted, #c0c0c0);\
             border:1px solid #644;border-radius:3px;cursor:pointer;font-size:12px"
        ).ok();

        // Stop handler.
        {
            let active_ref = live_active.clone();
            let status_ref = live_status.clone();
            let preview_ref = live_preview.clone();
            let stop_ref = stop_btn.clone();
            let start_ref = start_btn.clone();
            util::listen(&stop_btn, "click", move |_| {
                *active_ref.borrow_mut() = false;
                util::set_text(&status_ref, "Stopped");
                util::clear_children(&preview_ref);
                stop_ref.set_attribute("style", "display:none").ok();
                start_ref.set_attribute("style", "").ok();
            }, closures);
        }

        // Start handler.
        {
            let active_ref = live_active.clone();
            let status_ref = live_status.clone();
            let preview_ref = live_preview.clone();
            let log_ref = log_list.clone();
            let found_ref = found_codes.clone();
            let on_scan_ref = on_scan.clone();
            let stop_ref = stop_btn.clone();
            let start_ref = start_btn.clone();

            util::listen(&start_btn, "click", move |_| {
                *active_ref.borrow_mut() = true;
                util::clear_children(&preview_ref);
                stop_ref.set_attribute("style",
                    "display:inline-block;margin-left:4px;padding:4px 10px;background:#4a2a2a;\
                     color:var(--text-muted, #c0c0c0);border:1px solid #644;border-radius:3px;cursor:pointer;font-size:12px"
                ).ok();
                start_ref.set_attribute("style", "display:none").ok();

                start_live(
                    preview_ref.clone(), status_ref.clone(),
                    log_ref.clone(), found_ref.clone(), on_scan_ref.clone(),
                    active_ref.clone(),
                );
            }, closures);
        }

        let btn_row = util::create_element("div");
        btn_row.set_attribute("style", "display:flex;flex-wrap:wrap;gap:4px;margin:4px 0").ok();
        util::append(&btn_row, &start_btn);
        util::append(&btn_row, &stop_btn);

        util::append(&live_section, &btn_row);
        util::append(&live_section, &live_status);
        util::append(&live_section, &live_preview);
        util::append(container, &live_section);
    }

    // ---- Results log ----
    util::append(&log, &log_header);
    util::append(&log, &log_list);
    util::append(container, &log);
}

fn add_to_log(log_list: &web_sys::Element, found: &Rc<RefCell<Vec<String>>>, code: &str) {
    let mut codes = found.borrow_mut();

    // Count occurrences.
    let count = codes.iter().filter(|c| c.as_str() == code).count();
    codes.push(code.to_string());
    let total_unique = {
        let mut uniq: Vec<&str> = codes.iter().map(|s| s.as_str()).collect();
        uniq.sort();
        uniq.dedup();
        uniq.len()
    };
    drop(codes);

    if count == 0 {
        // New code — add entry.
        let entry = util::create_element("div");
        entry.set_attribute("data-code", code).ok();
        entry.set_attribute("style",
            "padding:3px 6px;margin:2px 0;background:#0a2a0a;border-radius:3px;color:var(--status-ok, #0f0);word-break:break-all"
        ).ok();
        util::set_text(&entry, code);
        log_list.append_child(&entry).ok();
    } else {
        // Duplicate — update counter on existing entry.
        if let Ok(Some(entry)) = log_list.query_selector(&format!("[data-code='{}']",
            code.replace('\'', "\\'")
        )) {
            util::set_text(&entry, &format!("{} (x{})", code, count + 1));
        }
    }

    // Update header with unique count.
    if let Some(parent) = log_list.parent_element() {
        if let Ok(Some(header)) = parent.query_selector("h4") {
            util::set_text(&header, &format!("Scanned Codes ({} unique)", total_unique));
        }
    }
}

fn start_live(
    container: web_sys::Element,
    status: web_sys::Element,
    log_list: web_sys::Element,
    found: Rc<RefCell<Vec<String>>>,
    on_scan: Rc<dyn Fn(String)>,
    active: Rc<RefCell<bool>>,
) {
    let video = util::document().create_element("video").unwrap()
        .dyn_into::<web_sys::HtmlVideoElement>().unwrap();
    video.set_attribute("playsinline", "true").ok();
    video.set_attribute("autoplay", "true").ok();
    video.set_attribute("muted", "true").ok();
    video.style().set_property("width", "100%").ok();
    video.style().set_property("max-width", "280px").ok();
    video.style().set_property("border-radius", "4px").ok();
    video.style().set_property("display", "block").ok();
    container.append_child(&video.clone().into()).ok();

    let navigator = web_sys::window().unwrap().navigator();
    let media_devices = match navigator.media_devices() {
        Ok(md) => md,
        Err(_) => { util::set_text(&status, "Camera not available"); return; }
    };

    let vc = js_sys::Object::new();
    js_sys::Reflect::set(&vc, &"facingMode".into(), &"environment".into()).ok();
    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_video(&vc);
    constraints.set_audio(&wasm_bindgen::JsValue::from_bool(false));

    let promise = match media_devices.get_user_media_with_constraints(&constraints) {
        Ok(p) => p,
        Err(_) => { util::set_text(&status, "Camera denied"); return; }
    };

    util::set_text(&status, "Starting camera...");

    wasm_bindgen_futures::spawn_local(async move {
        let stream_val = match JsFuture::from(promise).await {
            Ok(v) => v,
            Err(_) => { util::set_text(&status, "Camera denied"); return; }
        };
        let stream: web_sys::MediaStream = match stream_val.dyn_into() {
            Ok(s) => s,
            Err(_) => { util::set_text(&status, "Stream error"); return; }
        };

        video.set_src_object(Some(&stream));
        if let Ok(p) = video.play() { let _ = JsFuture::from(p).await; }

        // Brief delay for camera to settle.
        let delay = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window().unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 500).ok();
        });
        let _ = JsFuture::from(delay).await;

        util::set_text(&status, "Scanning...");

        let detector = match make_detector() {
            Some(d) => d,
            None => { util::set_text(&status, "Detector failed"); return; }
        };

        // Scan loop — uses Rc<Closure> for self-scheduling via setTimeout.
        // This is an intentional Rc cycle that lives until `active` is set to false.
        let scan_count = Rc::new(RefCell::new(0u32));
        let tick_fn: Rc<RefCell<Option<Closure<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let tick_fn_clone = tick_fn.clone();

        let tick = Closure::wrap(Box::new(move || {
            if !*active.borrow() {
                stop_stream(&stream);
                return;
            }

            let mut c = scan_count.borrow_mut();
            *c += 1;
            let n = *c;
            drop(c);

            let promise = match detect_from(&detector, &video) {
                Some(p) => p,
                None => {
                    util::set_text(&status, &format!("Scan {} — detect error", n));
                    schedule_next(&tick_fn_clone, 500);
                    return;
                }
            };

            let status_c = status.clone();
            let log_c = log_list.clone();
            let found_c = found.clone();
            let on_scan_c = on_scan.clone();
            let active_c = active.clone();
            let stream_c = stream.clone();
            let tick_ref = tick_fn_clone.clone();

            wasm_bindgen_futures::spawn_local(async move {
                match JsFuture::from(promise).await {
                    Ok(barcodes) => {
                        let arr: js_sys::Array = match barcodes.dyn_into() {
                            Ok(a) => a, Err(_) => js_sys::Array::new(),
                        };
                        if arr.length() > 0 {
                            let first = arr.get(0);
                            if let Ok(raw) = js_sys::Reflect::get(&first, &"rawValue".into()) {
                                if let Some(text) = raw.as_string() {
                                    add_to_log(&log_c, &found_c, &text);
                                    on_scan_c(text);
                                    util::set_text(&status_c, "Found! Continuing scan...");
                                    // Don't stop — keep scanning for more codes.
                                }
                            }
                        }
                        if *active_c.borrow() {
                            util::set_text(&status_c, &format!("Scan {}...", n));
                            schedule_next(&tick_ref, 300);
                        } else {
                            stop_stream(&stream_c);
                        }
                    }
                    Err(_) => {
                        util::set_text(&status_c, &format!("Scan {} — error", n));
                        schedule_next(&tick_ref, 500);
                    }
                }
            });
        }) as Box<dyn Fn()>);

        *tick_fn.borrow_mut() = Some(tick);
        schedule_next(&tick_fn, 300);
    });
}

fn schedule_next(tick_fn: &Rc<RefCell<Option<Closure<dyn Fn()>>>>, delay_ms: i32) {
    if let Some(ref cb) = *tick_fn.borrow() {
        if let Some(win) = web_sys::window() {
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(), delay_ms,
            );
        }
    }
}

fn stop_stream(stream: &web_sys::MediaStream) {
    let tracks = stream.get_tracks();
    for i in 0..tracks.length() {
        let track = tracks.get(i);
        if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
            track.stop();
        }
    }
}

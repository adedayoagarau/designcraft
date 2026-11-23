//! The browser shell: web `Services`, drag-and-drop, and the eframe web runner.

use super::browser_policy::{has_query_flag, mime_for, open_path, save_path};
use crate::graphics::{GraphicsState, RecoveryDocument, recovery_documents};
use designcraft_engine::Session;
use designcraft_ui_egui::{DesignApp, ImportRequest, ImportedFile, Inbox, Services};
use wasm_bindgen::JsCast as _;

const DOC_EXTS: &[&str] = &["designcraft", "idml", "indd", "indt"];
const IMAGE_EXTS: &[&str] = &[
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "tif",
    "tiff",
    "bmp",
    "psd",
    "svg",
    "pdf",
    "ai",
    "txt",
    "docx",
    "rtf",
    "md",
    "xlsx",
    "idml",
    "designcraft",
];
const CANVAS_ID: &str = "designcraft_canvas";
const LOADING_ID: &str = "designcraft_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        if let Err(e) = create_error_panel(&document) {
            log::error!("couldn't create graphics error panel: {e:?}");
            if let Some(el) = document.get_element_by_id(LOADING_ID) {
                el.set_text_content(Some("The editor couldn't start. Reload this page to try again."));
            }
            return;
        }
        let graphics = GraphicsState::default();
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            graphics_failed(&graphics, "The editor couldn't start", "Reload this page to try again.", &format!("Missing canvas {CANVAS_ID}."));
            return;
        };
        let mut options = eframe::WebOptions::default();
        if has_query_flag(&query(), "webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let runner = eframe::WebRunner::new();
        let creator_graphics = graphics.clone();
        let result = runner
            .start(
                canvas.clone(),
                options,
                Box::new(move |cc| {
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("designcraft-web: wgpu backend {:?}", rs.adapter.get_info().backend);
                        let device_graphics = creator_graphics.clone();
                        // This also observes an already-lost WebGPU device through its lost promise.
                        rs.device.set_device_lost_callback(move |reason, message| {
                            graphics_failed(&device_graphics, GRAPHICS_TITLE, GRAPHICS_MESSAGE, &format!("Device loss ({reason:?}): {message}"));
                        });
                    }
                    let inbox: Inbox = Inbox::default();
                    let errors: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
                    let unsaved: Unsaved = Default::default();
                    let opened = open_path(&query());
                    // Missa: a document opened from Missa saves back to it, not to the downloads.
                    let save_to = opened.as_ref().and_then(|(path, _)| save_path(path));
                    let mut app = DesignApp::new(Session::new(), services(inbox.clone(), cc.egui_ctx.clone(), save_to, errors.clone(), unsaved.clone()));
                    if has_query_flag(&query(), "sample") {
                        let _ = app.run("file.newSample", serde_json::json!({}));
                    }
                    // Missa: `?open=` loads the issue straight away, so the start screen only
                    // shows while it downloads, or with the reason it couldn't.
                    if let Some((path, name)) = opened {
                        let request = app.import_request("open");
                        let open_inbox = inbox.clone();
                        let ctx = cc.egui_ctx.clone();
                        let read_errors = errors.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            match fetch_bytes(&path).await {
                                Ok(bytes) => {
                                    load_missa_fonts(&name, &bytes).await;
                                    open_inbox.lock().unwrap_or_else(|e| e.into_inner()).push(ImportedFile { request, name, bytes });
                                }
                                Err(error) => {
                                    log::error!("{error}");
                                    read_errors.lock().unwrap_or_else(|e| e.into_inner()).push(format!("Couldn't open {name}: {error}"));
                                }
                            }
                            ctx.request_repaint();
                        });
                        // `fit`: a first layout from Missa. Once it opens, its pages follow its text.
                        let fit_pending = has_query_flag(&query(), "fit");
                        return Ok(Box::new(WebShell { app, inbox, read_errors: errors, graphics: creator_graphics, fit_pending, unsaved }));
                    }
                    Ok(Box::new(WebShell { app, inbox, read_errors: errors, graphics: creator_graphics, fit_pending: false, unsaved }))
                }),
            )
            .await;
        match result {
            Ok(()) => {
                if let Err(e) = install_recovery_controls(&runner, &canvas, &graphics) {
                    graphics_failed(&graphics, GRAPHICS_TITLE, GRAPHICS_MESSAGE, &format!("Couldn't install graphics recovery controls: {e:?}"));
                    if let Some(shell) = runner.app_mut::<WebShell>()
                        && let Err(e) = prepare_document_recovery(&runner, &shell.app.session)
                    {
                        recovery_controls_failed(&e);
                    }
                }
                // A loss callback can run during startup. Never hide its failure panel.
                if !graphics.has_failed()
                    && let Some(el) = document.get_element_by_id(LOADING_ID)
                {
                    el.remove();
                }
            }
            Err(e) => {
                graphics_failed(
                    &graphics,
                    "The editor couldn't start",
                    "Reload this page to try again. A browser with WebGPU or WebGL2 is required.",
                    &format!("{e:?}"),
                );
                if let Some(shell) = runner.app_mut::<WebShell>()
                    && let Err(e) = prepare_document_recovery(&runner, &shell.app.session)
                {
                    recovery_controls_failed(&e);
                }
            }
        }
    });
}

const ERROR_ID: &str = "designcraft_graphics_error";
const ERROR_TITLE_ID: &str = "designcraft_graphics_title";
const ERROR_MESSAGE_ID: &str = "designcraft_graphics_message";
const ERROR_DETAIL_ID: &str = "designcraft_graphics_detail";
const ERROR_ACTIONS_ID: &str = "designcraft_graphics_actions";
const ERROR_STATUS_ID: &str = "designcraft_graphics_status";
const ERROR_DOCUMENTS_ID: &str = "designcraft_graphics_documents";
const GRAPHICS_LOST_EVENT: &str = "designcraftgraphicslost";
const GRAPHICS_TITLE: &str = "Graphics connection lost";
const GRAPHICS_MESSAGE: &str = "Editing has stopped. Save a copy of each open document, then reload this page and reopen the copies. Reloading discards changes you have not downloaded.";

fn create_error_panel(document: &web_sys::Document) -> Result<(), wasm_bindgen::JsValue> {
    let panel: web_sys::HtmlElement = document.create_element("div")?.dyn_into()?;
    panel.set_id(ERROR_ID);
    panel.set_tab_index(-1);
    panel.set_attribute("role", "alertdialog")?;
    panel.set_attribute("aria-modal", "true")?;
    panel.set_attribute("aria-labelledby", ERROR_TITLE_ID)?;
    panel.set_attribute("aria-describedby", ERROR_MESSAGE_ID)?;
    // DOM content remains visible when the GPU can no longer paint the egui canvas.
    let colors = designcraft_ui_egui::theme::Tokens::for_brightness(designcraft_ui_egui::theme::Brightness::Dark);
    panel.style().set_css_text(&format!(
        "position:fixed;inset:0;z-index:1;display:none;flex-direction:column;align-items:center;justify-content:safe center;box-sizing:border-box;padding:24px;overflow:auto;background:{};color:{};font:14px system-ui,sans-serif;gap:12px;",
        css_color(colors.app_bar), css_color(colors.text),
    ));
    for (tag, id) in [("h1", ERROR_TITLE_ID), ("p", ERROR_MESSAGE_ID), ("div", ERROR_ACTIONS_ID), ("p", ERROR_STATUS_ID), ("pre", ERROR_DETAIL_ID)] {
        let child: web_sys::HtmlElement = document.create_element(tag)?.dyn_into()?;
        child.set_id(id);
        child.style().set_css_text("margin:0;max-width:640px;white-space:pre-wrap;overflow-wrap:anywhere;");
        if id == ERROR_ACTIONS_ID {
            child.style().set_css_text("display:flex;flex-direction:column;gap:8px;max-width:100%;");
            add_reload_control(document, &child)?;
        } else if id == ERROR_STATUS_ID {
            child.set_attribute("aria-live", "polite")?;
        }
        panel.append_child(&child)?;
    }
    document.body().ok_or_else(|| wasm_bindgen::JsValue::from_str("no body"))?.append_child(&panel)?;
    Ok(())
}

fn graphics_failed(graphics: &GraphicsState, title: &str, message: &str, detail: &str) {
    if !graphics.fail() {
        return;
    }
    log::error!("designcraft-web: {detail}");
    if let Err(e) = show_error(title, message, detail) {
        log::error!("couldn't show graphics error: {e:?}");
    }
    // A device callback may run while AppRunner is borrowed. Defer its notification;
    // the WebGL listener already holding AppRunner captures documents directly.
    if let Some(window) = web_sys::window()
        && let Some(canvas) = window.document().and_then(|doc| doc.get_element_by_id(CANVAS_ID))
    {
        let notify = wasm_bindgen::closure::Closure::once_into_js(move || {
            if let Err(e) = web_sys::Event::new(GRAPHICS_LOST_EVENT).and_then(|event| canvas.dispatch_event(&event)) {
                recovery_controls_failed(&e);
            }
        });
        window.queue_microtask(notify.unchecked_ref());
    }
}

fn show_error(title: &str, message: &str, detail: &str) -> Result<(), wasm_bindgen::JsValue> {
    let document = web_sys::window().and_then(|w| w.document()).ok_or_else(|| wasm_bindgen::JsValue::from_str("no document"))?;
    let panel: web_sys::HtmlElement =
        document.get_element_by_id(ERROR_ID).ok_or_else(|| wasm_bindgen::JsValue::from_str("no graphics error panel"))?.dyn_into()?;
    for (id, text) in [(ERROR_TITLE_ID, title), (ERROR_MESSAGE_ID, message), (ERROR_DETAIL_ID, detail)] {
        if let Some(el) = document.get_element_by_id(id) {
            el.set_text_content(Some(text));
        }
    }
    panel.style().set_property("display", "flex")?;
    if let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok()) {
        canvas.style().set_property("visibility", "hidden")?;
    }
    if let Some(loading) = document.get_element_by_id(LOADING_ID).and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok()) {
        loading.style().set_property("display", "none")?;
    }
    // Include eframe's hidden text input: neither pointer nor keyboard focus may reach the editor.
    let mut sibling = document.body().and_then(|body| body.first_element_child());
    while let Some(el) = sibling {
        sibling = el.next_element_sibling();
        if el.id() != ERROR_ID {
            if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
                html.set_inert(true);
            }
            el.set_attribute("aria-hidden", "true")?;
        }
    }
    panel.focus()?;
    Ok(())
}

/// Recovery listeners are owned by their DOM targets, independently of WebRunner.
fn add_dom_event_listener(
    target: &web_sys::EventTarget,
    event_name: &str,
    callback: impl FnMut(web_sys::Event) + 'static,
) -> Result<(), wasm_bindgen::JsValue> {
    let callback = wasm_bindgen::closure::Closure::wrap(Box::new(callback) as Box<dyn FnMut(web_sys::Event)>);
    target.add_event_listener_with_callback(event_name, callback.as_ref().unchecked_ref())?;
    // EventTarget retains the JS function; weak references reclaim its captures with the target.
    let _ = callback.into_js_value();
    Ok(())
}

fn css_color(color: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

fn recovery_button(document: &web_sys::Document) -> Result<web_sys::HtmlElement, wasm_bindgen::JsValue> {
    let button: web_sys::HtmlElement = document.create_element("button")?.dyn_into()?;
    button.set_attribute("type", "button")?;
    let colors = designcraft_ui_egui::theme::Tokens::for_brightness(designcraft_ui_egui::theme::Brightness::Dark);
    button.style().set_css_text(&format!(
        "box-sizing:border-box;max-width:100%;padding:6px 12px;border:1px solid {};border-radius:{}px;background:{};color:{};font:inherit;white-space:normal;overflow-wrap:anywhere;cursor:pointer;",
        css_color(colors.field_border), colors.radius, css_color(colors.button), css_color(colors.text),
    ));
    Ok(button)
}

fn add_reload_control(document: &web_sys::Document, actions: &web_sys::HtmlElement) -> Result<(), wasm_bindgen::JsValue> {
    let reload = recovery_button(document)?;
    reload.set_text_content(Some("Reload"));
    add_dom_event_listener(&reload, "click", move |event| {
        event.stop_propagation();
        if let Some(window) = web_sys::window()
            && let Err(e) = window.location().reload()
        {
            log::error!("couldn't reload: {e:?}");
        }
    })?;
    actions.append_child(&reload)?;
    Ok(())
}

fn install_recovery_controls(
    runner: &eframe::WebRunner,
    canvas: &web_sys::HtmlCanvasElement,
    graphics: &GraphicsState,
) -> Result<(), wasm_bindgen::JsValue> {
    let loss_runner = runner.clone();
    runner.add_event_listener(canvas, GRAPHICS_LOST_EVENT, move |_: web_sys::Event, app_runner| {
        let shell = app_runner.app_mut::<WebShell>();
        if shell.graphics.has_failed()
            && let Err(e) = prepare_document_recovery(&loss_runner, &shell.app.session)
        {
            recovery_controls_failed(&e);
        }
    })?;
    let context_graphics = graphics.clone();
    let context_runner = runner.clone();
    // wgpu's GLES backend does not observe browser WebGL context-loss events.
    runner.add_event_listener(canvas, "webglcontextlost", move |_: web_sys::Event, app_runner| {
        graphics_failed(&context_graphics, GRAPHICS_TITLE, GRAPHICS_MESSAGE, "The browser lost the WebGL graphics context.");
        if let Err(e) = prepare_document_recovery(&context_runner, &app_runner.app_mut::<WebShell>().app.session) {
            recovery_controls_failed(&e);
        }
    })?;

    // A WebGPU device can be lost while start() is still awaiting initialization.
    if graphics.has_failed()
        && let Some(shell) = runner.app_mut::<WebShell>()
    {
        prepare_document_recovery(runner, &shell.app.session)?;
    }
    Ok(())
}

fn prepare_document_recovery(runner: &eframe::WebRunner, session: &Session) -> Result<(), wasm_bindgen::JsValue> {
    let window = web_sys::window().ok_or_else(|| wasm_bindgen::JsValue::from_str("no window"))?;
    let documents = recovery_documents(session);
    if add_document_recovery_controls(documents)? {
        // Controls now own the snapshots. Release AppRunner's borrow before stopping painting
        // and removing its listeners/ResizeObserver; never discard documents on setup failure.
        let runner = runner.clone();
        let stop = wasm_bindgen::closure::Closure::once_into_js(move || runner.destroy());
        window.queue_microtask(stop.unchecked_ref());
    }
    Ok(())
}

fn recovery_controls_failed(error: &wasm_bindgen::JsValue) {
    log::error!("couldn't prepare recovery downloads: {error:?}");
    if let Some(status) = web_sys::window().and_then(|w| w.document()).and_then(|doc| doc.get_element_by_id(ERROR_STATUS_ID)) {
        status.set_text_content(Some("Couldn't prepare document downloads. Keep this page open to preserve unsaved documents."));
    }
}

fn add_document_recovery_controls(snapshots: Vec<RecoveryDocument>) -> Result<bool, wasm_bindgen::JsValue> {
    let document = web_sys::window().and_then(|w| w.document()).ok_or_else(|| wasm_bindgen::JsValue::from_str("no document"))?;
    let actions = document.get_element_by_id(ERROR_ACTIONS_ID).ok_or_else(|| wasm_bindgen::JsValue::from_str("no recovery actions"))?;
    if document.get_element_by_id(ERROR_DOCUMENTS_ID).is_some() {
        return Ok(false);
    }
    let documents: web_sys::HtmlElement = document.create_element("div")?.dyn_into()?;
    documents.set_id(ERROR_DOCUMENTS_ID);
    documents.style().set_css_text("display:flex;flex-direction:column;gap:8px;max-width:100%;");
    for snapshot in snapshots {
        let button = recovery_button(&document)?;
        button.set_text_content(Some(&format!("Save a Copy: {}", snapshot.title())));
        add_dom_event_listener(&button, "click", move |event| {
            event.stop_propagation();
            let result = snapshot.save_copy().and_then(|(name, bytes)| {
                download(&name, &bytes)?;
                Ok(format!("Requested download of {name}. Check your browser downloads before reloading."))
            });
            let text = result.unwrap_or_else(|e| e);
            if let Some(status) = web_sys::window().and_then(|w| w.document()).and_then(|doc| doc.get_element_by_id(ERROR_STATUS_ID)) {
                status.set_text_content(Some(&text));
            }
        })?;
        documents.append_child(&button)?;
    }
    // Build off-DOM so a setup error leaves the session intact and permits a clean retry.
    // The inserted element's id also prevents duplicate buttons on repeated loss notifications.
    actions.insert_before(&documents, actions.first_child().as_ref())?;
    Ok(true)
}

/// Fetch a same-origin file with the page's own sign-in (cookies), refusing any non-2xx reply.
/// Missa: before a document from Missa opens, add the typefaces it uses that the editor lacks,
/// from Missa's copies at `fonts/` beside the editor. A face that can't be fetched is left out,
/// and the Missing Fonts dialog names it when the document opens.
async fn load_missa_fonts(name: &str, bytes: &[u8]) {
    let families = crate::missa_fonts::missing_families(name, bytes);
    if families.is_empty() {
        return;
    }
    let manifest = match fetch_bytes("fonts/manifest.json").await {
        Ok(manifest) => serde_json::from_slice(&manifest).unwrap_or(serde_json::Value::Null),
        Err(error) => {
            log::warn!("Couldn't read Missa's typefaces: {error}");
            return;
        }
    };
    for file in crate::missa_fonts::files_for(&manifest, &families) {
        match fetch_bytes(&format!("fonts/{file}")).await {
            Ok(font) => {
                if designcraft_fonts::FontDb::global().add_font(font) == 0 {
                    log::warn!("{file} isn't a typeface the editor can read");
                }
            }
            Err(error) => log::warn!("Couldn't load {file}: {error}"),
        }
    }
}

async fn fetch_bytes(path: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let response =
        wasm_bindgen_futures::JsFuture::from(window.fetch_with_str_and_init(path, &init)).await.map_err(|e| format!("network error ({e:?})"))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| "unexpected reply".to_string())?;
    if !response.ok() {
        return Err(format!("the server answered {}", response.status()));
    }
    let buffer = response.array_buffer().map_err(|e| format!("couldn't read the reply ({e:?})"))?;
    let buffer = wasm_bindgen_futures::JsFuture::from(buffer).await.map_err(|e| format!("couldn't read the reply ({e:?})"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Missa: send a saved document back with the page's own sign-in. A refusal says why when the
/// server explains (`{"error": …}`).
async fn put_bytes(path: &str, bytes: &[u8]) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("PUT");
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let body = js_sys::Uint8Array::from(bytes);
    init.set_body(&body);
    let response =
        wasm_bindgen_futures::JsFuture::from(window.fetch_with_str_and_init(path, &init)).await.map_err(|_| "no connection".to_string())?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| "unexpected reply".to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    let text = match response.text() {
        Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
        Err(_) => String::new(),
    };
    let reason = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
    Err(reason.unwrap_or_else(|| format!("the server answered {status}")))
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// Wraps the app to read dropped files asynchronously (browsers can't read them synchronously)
/// and feed them through the inbox.
struct WebShell {
    app: DesignApp,
    inbox: Inbox,
    /// Why a dropped file couldn't be read, for the status bar (the read finishes outside a frame).
    read_errors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    graphics: GraphicsState,
    /// Missa: fit the issue opened with `?fit` once it has opened.
    fit_pending: bool,
    /// Missa: documents whose save to Missa failed after Save had already marked them saved.
    unsaved: Unsaved,
}

/// Names (`<title>.designcraft`) of documents whose save to Missa was refused or lost.
type Unsaved = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// Missa: Save marks a document saved as soon as it is handed over, but the PUT finishes later.
/// When it fails, mark the document unsaved again, so leaving or closing still warns.
fn mark_unsaved(app: &mut DesignApp, names: Vec<String>) {
    for name in names {
        let Some(index) = app.session.documents().iter().position(|d| d.path.as_deref() == Some(name.as_str())) else { continue };
        let back = app.session.active_index().filter(|&b| b != index);
        if back.is_some() {
            app.session.set_active(index);
        }
        if let Some(st) = app.session.active_mut()
            && !st.is_dirty()
        {
            // Dirty means the document differs from the saved allocation; a copy is never it.
            st.saved_doc = std::sync::Arc::new((*st.doc).clone());
            st.saved_revision = st.revision.wrapping_sub(1);
        }
        if let Some(back) = back {
            app.session.set_active(back);
        }
    }
}

/// Missa: a first layout comes with an estimate of its pages. Make its longest threaded story
/// the primary story, so Smart Text Reflow adds the pages its text needs and removes empty ones,
/// then bring the table of contents up to date with the pages the pieces start on.
fn fit_issue(app: &mut DesignApp) {
    let Some(st) = app.session.active() else { return };
    let d = &st.doc;
    let toc = d.toc.as_ref().map(|t| t.story);
    let body = d.stories.values().filter(|s| Some(s.id) != toc).max_by_key(|s| (s.frames.len(), s.text.len()));
    let Some(frame) = body.and_then(|s| s.frames.first().copied()) else { return };
    let has_toc = toc.is_some();
    let mut failed = Vec::new();
    if let Err(e) = app.run("object.primaryTextFrame", serde_json::json!({"on": true, "ids": [frame.0]})) {
        failed.push(e);
    }
    if has_toc && let Err(e) = app.run("toc.update", serde_json::json!({})) {
        failed.push(e);
    }
    match failed.first() {
        Some(e) => app.status(format!("Couldn’t fit the pages to the text: {e}")),
        None => app.status("Pages fitted to the text. Contents updated."),
    }
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let graphics = self.graphics.clone();
        let errors = std::mem::take(&mut *self.read_errors.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(error) = errors.into_iter().last() {
            self.app.status(error);
        }
        let unsaved = std::mem::take(&mut *self.unsaved.lock().unwrap_or_else(|e| e.into_inner()));
        if !unsaved.is_empty() {
            mark_unsaved(&mut self.app, unsaved);
        }
        let _ = graphics.run_if_healthy(|| self.app.logic(ctx));
        if self.fit_pending && self.app.session.active().is_some() {
            self.fit_pending = false;
            fit_issue(&mut self.app);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let _ = self.graphics.run_if_healthy(|| {
            // Dropped files belong to this host iteration's raw input, not to stale logic input:
            // read them here once, before the batch is saved as pending for a hidden pass.
            // Files dropped together arrive together, in their order, so several images load the
            // place cursor as one batch.
            let dropped = std::mem::take(&mut raw.dropped_files);
            if !dropped.is_empty() {
                let request = self.app.import_request("drop");
                let inbox = self.inbox.clone();
                let read_errors = self.read_errors.clone();
                let ctx = ctx.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let files = crate::browser_policy::read_batch(dropped, |f| {
                        let request = request.clone();
                        async move {
                            let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped".into());
                            let bytes = f.bytes_async().await.map_err(|e| format!("couldn't read dropped file {name}: {e}"))?;
                            Ok::<_, String>(ImportedFile { request, name, bytes })
                        }
                    })
                    .await;
                    match files {
                        Ok(files) => {
                            inbox.lock().unwrap_or_else(|e| e.into_inner()).extend(files);
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            log::error!("{error}");
                            read_errors.lock().unwrap_or_else(|e| e.into_inner()).push(error);
                            ctx.request_repaint();
                        }
                    }
                });
            }
            designcraft_ui_egui::drop_key_name_text(raw, ctx.text_edit_focused() || self.app.canvas_ime());
            self.app.raw_input_hook(ctx, raw);
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let _ = self.graphics.run_if_healthy(|| self.app.ui(ui));
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn services(inbox: Inbox, ctx: egui::Context, save_to: Option<String>, notices: std::sync::Arc<std::sync::Mutex<Vec<String>>>, unsaved: Unsaved) -> Services {
    let open_inbox = inbox.clone();
    let save_ctx = ctx.clone();
    // Save hands the document over as `<title>.designcraft`; from Missa it goes back to Missa.
    // Exports and other files still download.
    let hand_over = move |name: &str, bytes: &[u8]| -> Result<(), String> {
        let Some(target) = save_to.clone().filter(|_| name.to_ascii_lowercase().ends_with(".designcraft")) else {
            return download(name, bytes);
        };
        let bytes = bytes.to_vec();
        let notices = notices.clone();
        let unsaved = unsaved.clone();
        let name = name.to_string();
        let ctx = save_ctx.clone();
        notices.lock().unwrap_or_else(|e| e.into_inner()).push("Saving to Missa…".into());
        wasm_bindgen_futures::spawn_local(async move {
            let message = match put_bytes(&target, &bytes).await {
                Ok(()) => "Saved to Missa.".to_string(),
                Err(e) => {
                    unsaved.lock().unwrap_or_else(|e| e.into_inner()).push(name);
                    format!("Couldn’t save to Missa: {}. Your changes are still here. Save again to retry.", e.trim_end_matches('.'))
                }
            };
            notices.lock().unwrap_or_else(|e| e.into_inner()).push(message);
            ctx.request_repaint();
        });
        Ok(())
    };
    let save_download = std::rc::Rc::new(std::cell::RefCell::new(hand_over));
    let write_download = save_download.clone();
    Services {
        open_async: Some(Box::new(move |request: ImportRequest| {
            let inbox = open_inbox.clone();
            let ctx = ctx.clone();
            let dialog = if request.purpose == "swatches" {
                rfd::AsyncFileDialog::new().add_filter("Swatch Exchange (ASE)", &["ase"])
            } else if request.purpose == "shortcuts" {
                rfd::AsyncFileDialog::new().add_filter("Shortcut set (JSON, InDesign text)", &["json", "txt"])
            } else if request.purpose == "place" {
                rfd::AsyncFileDialog::new().add_filter("Graphics", IMAGE_EXTS)
            } else {
                rfd::AsyncFileDialog::new().add_filter("Missa, IDML or InDesign", DOC_EXTS)
            };
            wasm_bindgen_futures::spawn_local(async move {
                // Place takes several files, loaded into the place cursor in the order chosen.
                let handles = if request.purpose == "place" {
                    dialog.pick_files().await.unwrap_or_default()
                } else {
                    dialog.pick_file().await.into_iter().collect()
                };
                let mut files = Vec::with_capacity(handles.len());
                for file in handles {
                    let bytes = file.read().await;
                    files.push(ImportedFile { request: request.clone(), name: file.file_name(), bytes });
                }
                inbox.lock().unwrap_or_else(|e| e.into_inner()).extend(files);
                ctx.request_repaint();
            });
        })),
        write: Some(Box::new(move |name: &str, bytes: &[u8]| (write_download.borrow_mut())(name, bytes))),
        download: Some(Box::new(move |name: &str, bytes: &[u8]| (save_download.borrow_mut())(name, bytes))),
        inbox: Some(inbox),
        ..Default::default()
    }
}

/// Trigger a browser download of `bytes` named after the last component of `path`.
fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "designcraft".into());
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_for(&name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js)?.dyn_into().map_err(|_| "not an anchor")?;
    a.set_href(&url);
    a.set_download(&name);
    a.style().set_property("display", "none").map_err(js)?;
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(js)?;
    a.click();
    a.remove();
    // Revoke after the click has been dispatched; the download keeps its own reference.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        web_sys::Url::revoke_object_url(&url).ok();
    });
    // The download has started; a missed revoke only keeps the blob until the page closes.
    if let Err(e) = window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000) {
        log::warn!("could not schedule revoking the download URL: {}", js(e));
    }
    Ok(())
}

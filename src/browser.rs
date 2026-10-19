//! Everything that touches CEF (cef-rs's bindings to its C API: kept here, so
//! they could be swapped for others without the rest knowing).
//!
//! A page is drawn off screen, each frame handed over as a GPU texture (a dmabuf
//! on Linux) and copied into the window's own on the GPU, before CEF has its
//! buffer back. Pages talk to the app through window.salir (bridge.js): bound in
//! each page's process (the renderer: this program again), as process messages.

use crate::gpu::{Gpu, Layer, PAGE_FORMAT};
use cef::*;
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use winit::window::CursorIcon;

const BRIDGE: &str = include_str!("bridge.js");

// How long after a frame it's copied again (PageState::settle): its drawing through by then.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(40);

// Certificates trusted whatever their errors (an app's own server's, made for the run: its
// HTTPS, for HTTP/2), as DER: exactly these, nothing else.
static TRUSTED: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());

/// `pem` (one certificate) trusted from now on, for this process's pages.
pub fn trust_certificate(pem: &str) -> Result<(), String> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = base64(body.trim()).ok_or("not a PEM certificate")?;
    let mut trusted = TRUSTED.lock().unwrap();
    if !trusted.contains(&der) {
        trusted.push(der);
    }
    Ok(())
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let digits: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').map(value).collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, d)| n | (*d as u32) << (18 - 6 * i));
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    (!out.is_empty()).then_some(out)
}
const MESSAGE: &str = "salir";

/// This process, if it's one of CEF's others (a page's, the GPU's...): run as
/// that, its exit code. None: the app's (CEF's browser process).
pub fn subprocess() -> Option<i32> {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let is_browser = args.as_cmd_line().is_none_or(|cmd| cmd.has_switch(Some(&"type".into())) != 1);
    let code = execute_process(Some(args.as_main_args()), Some(&mut SalirApp::new()), std::ptr::null_mut());
    (!is_browser).then_some(code)
}

/// CEF started, its data (cache, cookies) in `cache`.
pub fn start(cache: &std::path::Path) -> bool {
    let args = args::Args::new();
    let settings = Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        no_sandbox: 1,
        root_cache_path: cache.to_string_lossy().as_ref().into(),
        log_severity: match std::env::var("SALIR_LOG").as_deref() {
            Ok("verbose") => LogSeverity::VERBOSE,
            Ok(_) => LogSeverity::INFO,
            Err(_) => LogSeverity::ERROR,
        },
        ..Default::default()
    };
    initialize(Some(args.as_main_args()), Some(&settings), Some(&mut SalirApp::new()), std::ptr::null_mut()) == 1
}

/// CEF's work since last asked (when it asked for it: app::schedule_pump, and often).
pub fn work() {
    do_message_loop_work();
    // And GLib's: Chromium watches its connections to the desktop (Wayland's, D-Bus) through
    // GLib's main loop on Linux, which the app has to run when it pumps CEF itself. (Without it a
    // direct window never hears from the compositor: it's never shown.)
    #[cfg(target_os = "linux")]
    for _ in 0..64 {
        if unsafe { g_main_context_iteration(std::ptr::null_mut(), 0) } == 0 {
            break;
        }
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn g_main_context_iteration(context: *mut std::ffi::c_void, may_block: i32) -> i32;
}

pub fn shutdown() {
    cef::shutdown();
}

wrap_app! {
    struct SalirApp;

    impl App {
        fn on_before_command_line_processing(&self, _process_type: Option<&CefStringUtf16>, command_line: Option<&mut CommandLine>) {
            let Some(command_line) = command_line else { return };
            // (And nothing to Google behind the page's back: Chrome's updates, pings, checks.)
            for switch in ["no-startup-window", "noerrdialogs", "hide-crash-restore-bubble", "disable-session-crashed-bubble", "disable-background-networking", "disable-component-update", "disable-domain-reliability", "disable-client-side-phishing-detection", "no-pings", "disable-sync"] {
                command_line.append_switch(Some(&switch.into()));
            }
            // Chrome's features that reach Google anyway: the Optimization Guide installs its
            // on-device AI models' manifest (a component, on demand: --disable-component-update
            // doesn't stop that); the clock checked against Google's; AI Mode's eligibility asked.
            // Added to the features switched off already (CEF's): a second switch would replace them.
            let off = ["OptimizationGuideManifestBroker", "OptimizationGuideModelExecution", "OptimizationHints", "OnDeviceModelBackgroundDownload", "NetworkTimeServiceQuerying", "AimServerEligibilityEnabled", "AimServerRequestOnStartupEnabled"];
            let name: CefString = "disable-features".into();
            let already = CefString::from(&command_line.switch_value(Some(&name))).to_string();
            let all = already.split(',').filter(|f| !f.is_empty()).chain(off).collect::<Vec<_>>().join(",");
            command_line.append_switch_with_value(Some(&name), Some(&all.as_str().into()));
            // And whatever else asks the component updater for something: its server's nowhere.
            command_line.append_switch_with_value(Some(&"component-updater".into()), Some(&"url-source=http://127.0.0.1:9/".into()));
            // SALIR_CHROMIUM_FLAGS="name=value name": Chromium's own switches, passed on.
            for switch in std::env::var("SALIR_CHROMIUM_FLAGS").unwrap_or_default().split_whitespace() {
                let switch = switch.trim_start_matches('-');
                match switch.split_once('=') {
                    Some((name, value)) => command_line.append_switch_with_value(Some(&name.into()), Some(&value.into())),
                    None => command_line.append_switch(Some(&switch.into())),
                }
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(Pump::new())
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(Bridge::new())
        }
    }
}

wrap_browser_process_handler! {
    struct Pump;

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            crate::app::schedule_pump(delay_ms);
        }
    }
}

// In a page's process: window.salir, its native half a function that sends a process message.
wrap_render_process_handler! {
    struct Bridge;

    impl RenderProcessHandler {
        fn on_context_created(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            let Some(context) = context else { return };
            let Some(global) = context.global() else { return };
            let Some(mut send) = v8_value_create_function(Some(&"__salirSend".into()), Some(&mut SendFn::new())) else { return };
            global.set_value_bykey(Some(&"__salirSend".into()), Some(&mut send), V8Propertyattribute::default());
            let (mut retval, mut exception) = (None, None);
            context.eval(Some(&BRIDGE.into()), Some(&"salir://bridge.js".into()), 1, Some(&mut retval), Some(&mut exception));
        }
    }
}

wrap_v8_handler! {
    struct SendFn;

    impl V8Handler {
        fn execute(&self, _name: Option<&CefString>, _object: Option<&mut V8Value>, arguments: Option<&[Option<V8Value>]>, _retval: Option<&mut Option<V8Value>>, _exception: Option<&mut CefString>) -> ::std::os::raw::c_int {
            let Some(text) = arguments.and_then(|a| a.first()).and_then(|a| a.as_ref()).map(|v| CefString::from(&v.string_value()).to_string()) else { return 0 };
            let Some(frame) = v8_context_get_current_context().and_then(|c| c.frame()) else { return 0 };
            let Some(mut message) = process_message_create(Some(&MESSAGE.into())) else { return 0 };
            if let Some(args) = message.argument_list() {
                args.set_string(0, Some(&text.as_str().into()));
            }
            frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
            1
        }
    }
}

/// What a window and its page share (all on the main thread: CEF's callbacks come
/// from work(), called there).
#[derive(Default)]
pub struct PageState {
    /// The page's size, in CSS pixels, and pixels to one of those.
    pub size: (i32, i32),
    pub scale: f32,
    /// What the page sent (window.salir), not yet seen to.
    pub messages: Vec<Value>,
    pub title: Option<String>,
    pub cursor: Option<CursorIcon>,
    /// window.open()'s and target=_blank's addresses (not opened: the app's to).
    pub popups: Vec<String>,
    /// Its last frame, our copy; and its popup's (a <select>'s list), where it is (CSS pixels).
    pub page: Option<(wgpu::Texture, Layer)>,
    pub popup: Option<(wgpu::Texture, Layer)>,
    pub popup_rect: Option<[i32; 4]>,
    /// Drawn again since last shown.
    pub painted: bool,
    pub closed: bool,
    /// Why a frame couldn't be shared (once).
    pub error: Option<String>,
    /// CEF's buffers, as imported (it draws into a few, in turn): by the dmabuf's identity.
    pub(crate) imported: HashMap<u64, Layer>,
    /// The buffers the page and its popup last came in (their keys in `imported`), and when to
    /// copy them again (settle): CEF hands a frame over with nothing to wait on, and copied
    /// there and then it can be the pool buffer as it last was, a frame from before (shown
    /// till the next: a <select>'s highlight a step behind, a panel shut a moment after it
    /// opened). Copied again a little later, it's the frame as drawn.
    pub(crate) last: [Option<u64>; 2],
    pub settle_at: Option<std::time::Instant>,
    /// A direct page's window (CEF's own), once made.
    pub(crate) window: Option<Window>,
    /// salir.files(): the paths asked for, by token, till the page's file dialog for them comes;
    /// a dialog that came first, waiting for them (the two come by different ways).
    pub(crate) files: HashMap<String, Vec<String>>,
    pub(crate) file_dialogs: HashMap<String, FileDialogCallback>,
}

impl PageState {
    /// Resized: CEF's buffers are new ones now.
    pub fn resized(&mut self) {
        self.imported.clear();
        self.last = [None, None];
    }

    /// The page's and its popup's last frames copied again (see `last`), now drawn through.
    pub fn settle(&mut self, gpu: &Gpu) {
        self.settle_at = None;
        for (i, key) in self.last.into_iter().enumerate() {
            let (Some(key), Some((texture, _))) = (key, if i == 0 { &self.page } else { &self.popup }) else { continue };
            if let Some(layer) = self.imported.get(&key) {
                gpu.copy(layer, texture);
                self.painted = true;
            }
        }
    }

    // Our copy of a frame (the page's, or its popup's), `width`×`height`.
    fn target(&mut self, gpu: &Gpu, popup: bool, width: u32, height: u32) -> &wgpu::Texture {
        let slot = if popup { &mut self.popup } else { &mut self.page };
        if slot.as_ref().is_none_or(|(t, _)| (t.width(), t.height()) != (width, height)) {
            let texture = gpu.target(width, height, PAGE_FORMAT);
            let layer = gpu.layer(&texture.create_view(&Default::default()));
            *slot = Some((texture, layer));
        }
        &slot.as_ref().unwrap().0
    }
}

#[derive(Clone)]
pub struct Shared {
    pub state: Rc<RefCell<PageState>>,
    pub gpu: Rc<Gpu>,
}

wrap_render_handler! {
    struct Renderer {
        shared: Shared,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (w, h) = self.shared.state.borrow().size;
                rect.width = w.max(1);
                rect.height = h.max(1);
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            let Some(screen_info) = screen_info else { return 0 };
            screen_info.device_scale_factor = self.shared.state.borrow().scale;
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            trace!("popup {}", if show != 0 { "shown" } else { "hidden" });
            if show == 0 {
                let mut state = self.shared.state.borrow_mut();
                state.popup_rect = None;
                state.popup = None;
                state.last[1] = None;
                state.painted = true;
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(r) = rect {
                trace!("popup at {} {}, {}×{}", r.x, r.y, r.width, r.height);
                self.shared.state.borrow_mut().popup_rect = Some([r.x, r.y, r.width, r.height]);
            }
        }

        #[cfg(target_os = "linux")]
        fn on_accelerated_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, _dirty_rects: Option<&[Rect]>, info: Option<&AcceleratedPaintInfo>) {
            use crate::dmabuf::{self, Dmabuf};
            let Some(info) = info else { return };
            let plane = &info.planes[0];
            let Some(key) = dmabuf::identity(plane.fd) else { return };
            let (width, height) = (info.extra.coded_size.width as u32, info.extra.coded_size.height as u32);
            let gpu = &self.shared.gpu;
            let mut state = self.shared.state.borrow_mut();
            if let std::collections::hash_map::Entry::Vacant(slot) = state.imported.entry(key) {
                let format = if info.format == ColorType::RGBA_8888 { wgpu::TextureFormat::Rgba8Unorm } else { wgpu::TextureFormat::Bgra8Unorm };
                match dmabuf::import(&gpu.device, &Dmabuf { fd: plane.fd, width, height, stride: plane.stride, offset: plane.offset as u32, modifier: info.modifier, format }) {
                    Ok(texture) => {
                        slot.insert(gpu.layer(&texture.create_view(&Default::default())));
                    }
                    Err(e) => {
                        state.error.get_or_insert(e);
                        return;
                    }
                }
            }
            let target = state.target(gpu, type_ == PaintElementType::POPUP, width, height).clone();
            gpu.copy(&state.imported[&key], &target);
            state.painted = true;
            state.last[(type_ == PaintElementType::POPUP) as usize] = Some(key);
            state.settle_at = Some(std::time::Instant::now() + SETTLE);
            trace!("{} painted, shared: {width}×{height}", if type_ == PaintElementType::POPUP { "popup" } else { "page" });
        }

        fn on_paint(&self, _browser: Option<&mut Browser>, type_: PaintElementType, _dirty_rects: Option<&[Rect]>, buffer: *const u8, width: ::std::os::raw::c_int, height: ::std::os::raw::c_int) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let (w, h) = (width as u32, height as u32);
            let bgra = unsafe { std::slice::from_raw_parts(buffer, (w * h * 4) as usize) };
            let gpu = &self.shared.gpu;
            let mut state = self.shared.state.borrow_mut();
            let target = state.target(gpu, type_ == PaintElementType::POPUP, w, h);
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bgra,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            state.painted = true;
            trace!("page painted, through memory: {w}×{h}");
        }
    }
}

wrap_display_handler! {
    struct Display {
        shared: Shared,
    }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            self.shared.state.borrow_mut().title = title.map(|t| t.to_string());
        }

        fn on_cursor_change(&self, _browser: Option<&mut Browser>, _cursor: ::std::os::raw::c_ulong, type_: CursorType, _custom_cursor_info: Option<&CursorInfo>) -> ::std::os::raw::c_int {
            let mut state = self.shared.state.borrow_mut();
            if state.window.is_some() {
                return 0; // (a direct page's window: Chromium's, which sets its cursor itself)
            }
            state.cursor = Some(cursor_icon(type_));
            1
        }

        fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity, message: Option<&CefString>, source: Option<&CefString>, line: ::std::os::raw::c_int) -> ::std::os::raw::c_int {
            // SALIR_CONSOLE=1: pages' console, on stderr.
            if std::env::var_os("SALIR_CONSOLE").is_some() {
                eprintln!("console: {} ({}:{line})", message.map(|m| m.to_string()).unwrap_or_default(), source.map(|s| s.to_string()).unwrap_or_default());
            }
            1
        }
    }
}

wrap_life_span_handler! {
    struct LifeSpan {
        shared: Shared,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            self.shared.state.borrow_mut().popups.push(target_url.map(|u| u.to_string()).unwrap_or_default());
            1 // (not opened)
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.shared.state.borrow_mut().closed = true;
        }
    }
}

wrap_request_handler! {
    struct Requests;

    impl RequestHandler {
        // A certificate error let through for a trusted certificate (trust_certificate), the very one.
        fn on_certificate_error(&self, _browser: Option<&mut Browser>, _cert_error: Errorcode, _request_url: Option<&CefString>, ssl_info: Option<&mut Sslinfo>, callback: Option<&mut Callback>) -> ::std::os::raw::c_int {
            let der = ssl_info.and_then(|i| i.x509_certificate()).and_then(|c| c.derencoded()).map(|b| {
                let mut buf = vec![0u8; b.size()];
                b.data(Some(&mut buf), 0);
                buf
            });
            match (der, callback) {
                (Some(der), Some(callback)) if TRUSTED.lock().unwrap().contains(&der) => {
                    callback.cont();
                    1
                }
                _ => 0, // (refused, as ever)
            }
        }
    }
}

// salir.files(paths): the page's file dialog (bridge.js: an <input type=file>, its accept a token)
// answered with those paths, unseen: the page has them as File objects (readable by it, and
// dragged out as files), as if picked. Other dialogs: CEF's.
const FILES_TOKEN: &str = ".salir-files-";

wrap_dialog_handler! {
    struct Dialogs {
        shared: Shared,
    }

    impl DialogHandler {
        fn on_file_dialog(&self, _browser: Option<&mut Browser>, _mode: FileDialogMode, _title: Option<&CefString>, _default_file_path: Option<&CefString>, accept_filters: Option<&mut CefStringList>, _accept_extensions: Option<&mut CefStringList>, _accept_descriptions: Option<&mut CefStringList>, callback: Option<&mut FileDialogCallback>) -> ::std::os::raw::c_int {
            let filters: Vec<String> = accept_filters.map(|l| CefStringList::from(<*mut cef::sys::_cef_string_list_t>::from(l)).into_iter().collect()).unwrap_or_default();
            let Some(token) = filters.iter().find_map(|f| f.strip_prefix(FILES_TOKEN)) else { return 0 };
            let Some(callback) = callback else { return 0 };
            let mut s = self.shared.state.borrow_mut();
            match s.files.remove(token) {
                Some(paths) => give_files(callback, &paths),
                None => {
                    s.file_dialogs.insert(token.to_string(), callback.clone());
                }
            }
            1
        }
    }
}

fn give_files(callback: &FileDialogCallback, paths: &[String]) {
    if paths.is_empty() {
        return callback.cancel();
    }
    let mut list = CefStringList::new();
    for p in paths {
        list.append(p);
    }
    callback.cont(Some(&mut list));
}

// "scheme://host:port" of a URL (the part a page's origin is).
fn origin(url: &str) -> &str {
    match url.find("://") {
        Some(i) => &url[..url[i + 3..].find('/').map_or(url.len(), |j| i + 3 + j)],
        None => url,
    }
}

// No context menu of CEF's (it would need a window of its own): pages make their own.
wrap_context_menu_handler! {
    struct NoMenu;

    impl ContextMenuHandler {
        fn on_before_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            if let Some(model) = model {
                model.clear();
            }
        }
    }
}

wrap_client! {
    struct SalirClient {
        shared: Shared,
        renderer: Option<RenderHandler>,
        display: DisplayHandler,
        life_span: LifeSpanHandler,
        menu: ContextMenuHandler,
        requests: RequestHandler,
        dialogs: DialogHandler,
        files_origin: Option<String>, // salir.files() allowed (WindowOptions::files): from this origin's top frame
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            self.renderer.clone() // (None: a direct page, drawing into its own window)
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.menu.clone())
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.requests.clone())
        }
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialogs.clone())
        }

        fn on_process_message_received(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _source_process: ProcessId, message: Option<&mut ProcessMessage>) -> ::std::os::raw::c_int {
            let Some(message) = message else { return 0 };
            if CefString::from(&message.name()).to_string() != MESSAGE {
                return 0;
            }
            let Some(text) = message.argument_list().map(|a| CefString::from(&a.string(0)).to_string()) else { return 1 };
            let Ok(value) = serde_json::from_str::<Value>(&text) else { return 1 };
            if value.get("$").and_then(Value::as_str) == Some("files") {
                // salir.files(): its own page's (top frame, its origin), if the window allows it.
                let url = frame.as_ref().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
                let allowed = self.files_origin.as_deref().is_some_and(|o| frame.as_ref().is_some_and(|f| f.is_main() == 1) && origin(&url) == o);
                let token = value.get("token").and_then(Value::as_str).unwrap_or("").to_string();
                let paths: Vec<String> = if allowed { value.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect()).unwrap_or_default() } else { vec![] };
                let mut s = self.shared.state.borrow_mut();
                match s.file_dialogs.remove(&token) {
                    Some(callback) => give_files(&callback, &paths),
                    None => {
                        s.files.insert(token, paths);
                    }
                }
                return 1;
            }
            self.shared.state.borrow_mut().messages.push(value);
            1
        }
    }
}

/// A direct page's window: CEF's own, Chromium drawing into it and showing it (no
/// compositing: as a browser does), none of salir's embeds.
pub struct DirectWindow {
    pub title: Option<String>,
    pub width: i32,
    pub height: i32,
    pub app_id: Option<String>,
}

/// A page, and what's done to it: off screen (drawn into the window salir composes), or
/// direct (in a window of its own).
pub struct Page {
    browser: Option<Browser>,
    view: Option<BrowserView>, // (a direct page's: its browser comes later)
    shared: Shared,
}

impl Page {
    /// A page at `url`; off screen, its frames handed over on the GPU where salir can take
    /// them so; or, `direct`, in a window of its own. `background`: ARGB, under the page.
    pub fn open(shared: &Shared, url: &str, direct: Option<DirectWindow>, background: u32, files: bool) -> Option<Page> {
        let settings = BrowserSettings {
            windowless_frame_rate: 60,
            background_color: if direct.is_some() { background } else { 0 }, // (off screen: see-through where the page is, embeds show there)
            ..Default::default()
        };
        let renderer = direct.is_none().then(|| Renderer::new(shared.clone()));
        let files_origin = files.then(|| origin(url).to_string());
        let mut client = SalirClient::new(shared.clone(), renderer, Display::new(shared.clone()), LifeSpan::new(shared.clone()), NoMenu::new(), Requests::new(), Dialogs::new(shared.clone()), files_origin);
        if let Some(direct) = direct {
            let view = browser_view_create(Some(&mut client), Some(&url.into()), Some(&settings), None, None, Some(&mut PlainView::new()))?;
            window_create_top_level(Some(&mut DirectDelegate::new(view.clone(), shared.clone(), direct.title, direct.width, direct.height, direct.app_id)))?;
            return Some(Page { browser: None, view: Some(view), shared: shared.clone() });
        }
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            shared_texture_enabled: cfg!(target_os = "linux") as _,
            ..Default::default()
        };
        browser_host_create_browser_sync(Some(&window_info), Some(&mut client), Some(&url.into()), Some(&settings), None, None).map(|browser| Page { browser: Some(browser), view: None, shared: shared.clone() })
    }

    fn browser(&self) -> Option<Browser> {
        self.browser.clone().or_else(|| self.view.as_ref()?.browser())
    }

    /// A direct page's title shown (its window's).
    pub fn set_title(&self, title: &str) {
        if let Some(w) = &self.shared.state.borrow().window {
            w.set_title(Some(&title.into()));
        }
    }

    fn host(&self) -> Option<BrowserHost> {
        self.browser()?.host()
    }

    /// Runs `code` in the page.
    pub fn run(&self, code: &str) {
        if let Some(frame) = self.browser().and_then(|b| b.main_frame()) {
            frame.execute_java_script(Some(&code.into()), None, 0);
        }
    }

    /// To the page's window.salir (its _receive).
    pub fn deliver(&self, message: &Value) {
        self.run(&format!("window.salir && salir._receive({message})"));
    }

    pub fn resized(&self) {
        if let Some(h) = self.host() {
            h.was_resized();
        }
    }

    pub fn screen_changed(&self) {
        if let Some(h) = self.host() {
            h.notify_screen_info_changed();
        }
    }

    pub fn focus(&self, focused: bool) {
        if let Some(h) = self.host() {
            h.set_focus(focused as _);
        }
    }

    /// Closed (its unload handlers run first): PageState.closed once it is.
    pub fn close(&self) {
        let window = self.shared.state.borrow().window.clone();
        match (window, self.host()) {
            (Some(w), _) => w.close(), // (a direct page's window: it asks the page, can_close)
            (None, Some(h)) => h.close_browser(0),
            (None, None) => self.shared.state.borrow_mut().closed = true,
        }
    }

    pub fn mouse_move(&self, x: i32, y: i32, flags: u32, leave: bool) {
        if let Some(h) = self.host() {
            h.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers: flags }), leave as _);
        }
    }

    /// `button`: 0 left, 1 middle, 2 right.
    pub fn mouse_button(&self, x: i32, y: i32, flags: u32, button: u8, up: bool, clicks: i32) {
        let button = match button {
            1 => MouseButtonType::MIDDLE,
            2 => MouseButtonType::RIGHT,
            _ => MouseButtonType::LEFT,
        };
        if let Some(h) = self.host() {
            h.send_mouse_click_event(Some(&MouseEvent { x, y, modifiers: flags }), button, up as _, clicks);
        }
    }

    pub fn wheel(&self, x: i32, y: i32, flags: u32, dx: i32, dy: i32) {
        if let Some(h) = self.host() {
            h.send_mouse_wheel_event(Some(&MouseEvent { x, y, modifiers: flags }), dx, dy);
        }
    }

    /// A key down or up: its Windows key code (keys.rs) and the system's.
    pub fn key(&self, down: bool, windows_code: i32, native_code: i32, flags: u32) {
        let event = KeyEvent {
            size: std::mem::size_of::<KeyEvent>(),
            type_: if down { KeyEventType::RAWKEYDOWN } else { KeyEventType::KEYUP },
            modifiers: flags,
            windows_key_code: windows_code,
            native_key_code: native_code,
            ..Default::default()
        };
        if let Some(h) = self.host() {
            h.send_key_event(Some(&event));
        }
    }

    /// A character typed as a key does (Enter's: a new line in a text area).
    pub fn char(&self, code: u16, windows_code: i32, flags: u32) {
        let event = KeyEvent { size: std::mem::size_of::<KeyEvent>(), type_: KeyEventType::CHAR, modifiers: flags, windows_key_code: windows_code, character: code, unmodified_character: code, ..Default::default() };
        if let Some(h) = self.host() {
            h.send_key_event(Some(&event));
        }
    }

    /// Text typed. (As text, not as keys: CEF on Linux would make each character from a
    /// key's Windows code, US style; this way any text comes through: é, þ, 字.)
    pub fn text(&self, text: &str) {
        if let Some(h) = self.host() {
            h.ime_commit_text(Some(&text.into()), Some(&Range { from: u32::MAX, to: u32::MAX }), 0);
        }
    }
}

// A direct page's view: plain (no browser around it: no toolbar, no tabs).
wrap_browser_view_delegate! {
    struct PlainView {} // (not `struct PlainView;`: cef-rs's macro loses the ViewDelegate block that way)

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }
    }
}

wrap_window_delegate! {
    struct DirectDelegate {
        view: BrowserView,
        shared: Shared,
        title: Option<String>,
        width: i32,
        height: i32,
        app_id: Option<String>,
    }

    impl ViewDelegate {
        // Its size to begin with (the page's view has none of its own).
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: self.width, height: self.height }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            Rect { x: 0, y: 0, width: self.width, height: self.height }
        }

        // Resizable (else its size is fixed: min = max, and a tiling compositor floats it).
        fn can_resize(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            1
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            1
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            1
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else { return };
            trace!("direct window made");
            window.add_child_view(Some(&mut View::from(&self.view)));
            window.set_title(Some(&self.title.as_deref().unwrap_or("salir").into()));
            window.show();
            self.view.request_focus();
            self.shared.state.borrow_mut().window = Some(window.clone());
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            trace!("direct window gone");
            let mut state = self.shared.state.borrow_mut();
            state.window = None;
            state.closed = true;
        }

        // Closed only once the page agrees (its beforeunload).
        fn can_close(&self, _window: Option<&mut Window>) -> ::std::os::raw::c_int {
            self.view.browser().and_then(|b| b.host()).map_or(1, |h| h.try_close_browser())
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }

        fn linux_window_properties(&self, _window: Option<&mut Window>, properties: Option<&mut LinuxWindowProperties>) -> ::std::os::raw::c_int {
            let (Some(app_id), Some(p)) = (&self.app_id, properties) else { return 0 };
            p.wayland_app_id = lasting_string(app_id);
            p.wm_class_class = lasting_string(app_id);
            p.wm_class_name = lasting_string(app_id);
            1
        }
    }
}

// A string for CEF to read in a struct salir fills in (an out-parameter): one cef-rs owns comes out
// empty there (its conversion to CEF's keeps only strings it borrows), so this one's CEF's own
// kind, borrowed, over UTF-16 that lasts (a few bytes, once per window: CEF copies it).
fn lasting_string(s: &str) -> CefString {
    let utf16: &'static [u16] = Box::leak(s.encode_utf16().collect::<Vec<u16>>().into_boxed_slice());
    let raw = cef::sys::_cef_string_utf16_t { str_: utf16.as_ptr() as *mut u16, length: utf16.len(), dtor: None };
    CefString::from(&raw as *const cef::sys::_cef_string_utf16_t)
}

/// CEF's event flags.
pub mod flags {
    pub const SHIFT: u32 = 1 << 1;
    pub const CONTROL: u32 = 1 << 2;
    pub const ALT: u32 = 1 << 3;
    pub const LEFT: u32 = 1 << 4;
    pub const MIDDLE: u32 = 1 << 5;
    pub const RIGHT: u32 = 1 << 6;
    pub const META: u32 = 1 << 7; // (EVENTFLAG_COMMAND_DOWN)
}

fn cursor_icon(t: CursorType) -> CursorIcon {
    match t {
        CursorType::HAND => CursorIcon::Pointer,
        CursorType::IBEAM => CursorIcon::Text,
        CursorType::VERTICALTEXT => CursorIcon::VerticalText,
        CursorType::CROSS => CursorIcon::Crosshair,
        CursorType::WAIT => CursorIcon::Wait,
        CursorType::PROGRESS => CursorIcon::Progress,
        CursorType::HELP => CursorIcon::Help,
        CursorType::MOVE => CursorIcon::Move,
        CursorType::CELL => CursorIcon::Cell,
        CursorType::CONTEXTMENU => CursorIcon::ContextMenu,
        CursorType::ALIAS => CursorIcon::Alias,
        CursorType::COPY => CursorIcon::Copy,
        CursorType::NODROP | CursorType::NOTALLOWED => CursorIcon::NotAllowed,
        CursorType::GRAB => CursorIcon::Grab,
        CursorType::GRABBING => CursorIcon::Grabbing,
        CursorType::ZOOMIN => CursorIcon::ZoomIn,
        CursorType::ZOOMOUT => CursorIcon::ZoomOut,
        CursorType::EASTRESIZE => CursorIcon::EResize,
        CursorType::WESTRESIZE => CursorIcon::WResize,
        CursorType::NORTHRESIZE => CursorIcon::NResize,
        CursorType::SOUTHRESIZE => CursorIcon::SResize,
        CursorType::NORTHEASTRESIZE => CursorIcon::NeResize,
        CursorType::NORTHWESTRESIZE => CursorIcon::NwResize,
        CursorType::SOUTHEASTRESIZE => CursorIcon::SeResize,
        CursorType::SOUTHWESTRESIZE => CursorIcon::SwResize,
        CursorType::EASTWESTRESIZE => CursorIcon::EwResize,
        CursorType::NORTHSOUTHRESIZE => CursorIcon::NsResize,
        CursorType::NORTHEASTSOUTHWESTRESIZE => CursorIcon::NeswResize,
        CursorType::NORTHWESTSOUTHEASTRESIZE => CursorIcon::NwseResize,
        CursorType::COLUMNRESIZE => CursorIcon::ColResize,
        CursorType::ROWRESIZE => CursorIcon::RowResize,
        CursorType::MIDDLEPANNING => CursorIcon::AllScroll,
        _ => CursorIcon::Default,
    }
}

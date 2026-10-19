//! The app: its windows, each a page over its embeds, put together on the GPU
//! when either changes; the mouse and keys to the page; CEF's work done when
//! CEF asks for it (its message pump: external, in winit's loop).

use crate::browser::{self, flags, Page, PageState, Shared};
use crate::control::{self, Command};
use crate::embed::{EmbedFactory, Frame, GlEmbedFactory, GlFrame, Link};
use crate::gpu::{Gpu, Layer, Surface};
use crate::{keys, Salir, WindowOptions};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// SALIR_TRACE=1: what salir does, on stderr.
macro_rules! trace {
    ($($t:tt)*) => {
        if *crate::app::TRACE.get_or_init(|| std::env::var_os("SALIR_TRACE").is_some()) {
            eprintln!("salir: {}", format!($($t)*));
        }
    };
}
pub(crate) static TRACE: OnceLock<bool> = OnceLock::new();

pub enum Event {
    /// CEF asked for its work to be done (schedule_pump).
    Pump,
    EmbedSend { window: u64, embed: u64, message: Value },
    EmbedFrame { window: u64, embed: u64 },
    EmbedRelease { window: u64, embed: u64 },
    Control(Command),
    ControlClosed,
}

// When CEF wants its work done next; and how to wake the loop for it (CEF asks from any thread).
static PUMP_AT: Mutex<Option<Instant>> = Mutex::new(None);
static PUMPED: Mutex<Option<Instant>> = Mutex::new(None); // (last done)
static PROXY: OnceLock<Mutex<EventLoopProxy<Event>>> = OnceLock::new();
const PUMP_AT_LEAST: Duration = Duration::from_millis(33); // (CEF's advice: every 1/30 s at the longest)
// With a direct window open: often. (Its input comes to Chromium, from its own window, only as
// its work is done; CEF doesn't ask for that.)
const PUMP_DIRECT: Duration = Duration::from_millis(4);

pub fn schedule_pump(delay_ms: i64) {
    let at = Instant::now() + Duration::from_millis(delay_ms.max(0) as u64);
    let mut next = PUMP_AT.lock().unwrap();
    if next.is_none_or(|n| at < n) {
        *next = Some(at);
    }
    drop(next);
    if let Some(proxy) = PROXY.get() {
        let _ = proxy.lock().unwrap().send_event(Event::Pump);
    }
}

/// GL's functions by name (plugins' get_proc_address).
pub fn gl_proc_address(name: &str) -> *const std::ffi::c_void {
    #[cfg(target_os = "linux")]
    return crate::gl::proc_address(name);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        std::ptr::null()
    }
}

pub fn run(salir: Salir) -> std::process::ExitCode {
    if let Some(code) = browser::subprocess() {
        return std::process::ExitCode::from(code.clamp(0, 255) as u8);
    }
    let cache = salir.data.clone().unwrap_or_else(|| std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")).join(&salir.name));
    if !browser::start(&cache) {
        eprintln!("salir: CEF didn't start");
        return std::process::ExitCode::FAILURE;
    }
    let gpu = match Gpu::new() {
        Ok(gpu) => Rc::new(gpu),
        Err(e) => {
            eprintln!("salir: {e}");
            browser::shutdown();
            return std::process::ExitCode::FAILURE;
        }
    };
    let event_loop = EventLoop::<Event>::with_user_event().build().expect("an event loop");
    let proxy = event_loop.create_proxy();
    let _ = PROXY.set(Mutex::new(proxy.clone()));
    let mut gl_embeds = salir.gl_embeds;
    for (kind, path) in &salir.plugins {
        match crate::plugin::load(path) {
            Ok(factory) => {
                gl_embeds.insert(kind.clone(), factory);
            }
            Err(e) => eprintln!("salir: plugin {kind}: {e}"),
        }
    }
    if salir.control {
        control::listen(proxy.clone());
        control::emit(json!({ "ready": true }));
    }
    let mut app = App {
        gpu,
        #[cfg(target_os = "linux")]
        gl: None,
        proxy,
        embeds: salir.embeds,
        gl_embeds,
        on_message: salir.on_message,
        control: salir.control,
        pending: salir.windows,
        windows: HashMap::new(),
        next_key: 1,
        quitting: false,
        dying: VecDeque::new(),
        turn: 0,
    };
    let _ = event_loop.run_app(&mut app);
    drop(app); // (every page gone before CEF is)
    browser::shutdown();
    std::process::ExitCode::SUCCESS
}

struct App {
    gpu: Rc<Gpu>,
    #[cfg(target_os = "linux")]
    gl: Option<Result<Rc<crate::gl::Gl>, String>>, // (made when an embed first needs it)
    proxy: EventLoopProxy<Event>,
    embeds: HashMap<String, EmbedFactory>,
    gl_embeds: HashMap<String, GlEmbedFactory>,
    on_message: Option<crate::OnMessage>,
    control: bool,
    pending: Vec<WindowOptions>,
    windows: HashMap<u64, Win>,
    next_key: u64,
    quitting: bool,
    // Embeds gone from their windows, still to be torn down (an mpv's takes up to 0.4 s): one
    // a turn of the event loop, from the turn after (a closed window's off the screen first,
    // the other windows answering in between). With the turn they went.
    dying: VecDeque<(u64, EmbedLayer)>,
    turn: u64,
}

struct Win {
    key: u64,
    id: String,
    surface: Option<Surface>, // (None: a direct window, CEF's own)
    page: Option<Page>,
    shared: Shared,
    embeds: Vec<EmbedLayer>,
    fixed_title: bool,
    background: wgpu::Color,
    // The mouse (CSS pixels), its buttons (CEF's flags), the keyboard's modifiers; the last
    // click, for double ones: when, which, where, how many.
    cursor: (f64, f64),
    buttons: u32,
    modifiers: ModifiersState,
    last_click: Option<(Instant, u8, (f64, f64), i32)>,
    closing: bool,
    /// The embed the window's input goes to directly (input="direct"), and whether the pointer's locked.
    capture: Option<(u64, bool)>,
    focused: bool,
}

impl Win {
    fn window(&self) -> Option<&Arc<Window>> {
        self.surface.as_ref().map(|s| &s.window)
    }

    fn redraw(&self) {
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }

    // The pointer locked and hidden (FPS-style; else just kept in the window), or let go.
    fn grab(&self, lock: bool) {
        let Some(w) = self.window() else { return };
        if lock {
            if w.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                let _ = w.set_cursor_grab(CursorGrabMode::Confined);
            }
            w.set_cursor_visible(false);
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
        }
    }

    // The input-capturing embed's place (CSS pixels): where its input's coordinates start.
    fn capture_origin(&self) -> (f32, f32) {
        self.capture.and_then(|(id, _)| self.embeds.iter().find(|e| e.id == id)).and_then(|e| e.rect).map_or((0.0, 0.0), |r| (r[0], r[1]))
    }

    fn capture_input(&mut self, input: &crate::Input) {
        let Some((id, _)) = self.capture else { return };
        if let Some(e) = self.embeds.iter_mut().find(|e| e.id == id) {
            match &mut e.imp {
                Imp::Wgpu { embed, .. } => embed.input(input),
                #[cfg(target_os = "linux")]
                Imp::Gl { embed, .. } => embed.input(input),
            }
        }
    }

    // The input back to the page; `tell`: the page told (it didn't ask).
    fn release(&mut self, tell: bool) {
        let Some((id, lock)) = self.capture.take() else { return };
        if lock {
            self.grab(false);
        }
        if tell && let Some(page) = &self.page {
            page.deliver(&json!({ "$": "embed.released", "id": id }));
        }
    }
}

struct EmbedLayer {
    id: u64,
    rect: Option<[f32; 4]>, // CSS pixels; None: not shown
    wants_frame: bool,
    imp: Imp,
}

enum Imp {
    Wgpu { embed: Box<dyn crate::Embed>, target: Option<(wgpu::Texture, Layer)> },
    #[cfg(target_os = "linux")]
    Gl { embed: Box<dyn crate::GlEmbed>, buffers: Vec<(crate::gl::Buffer, Layer)>, next: usize, shown: Option<usize> },
}

// How many buffers a GL embed draws into in turn (one shown, one maybe still read, one drawn).
#[cfg(target_os = "linux")]
const GL_BUFFERS: usize = 3;

impl App {
    fn open(&mut self, event_loop: &ActiveEventLoop, options: WindowOptions) {
        if self.windows.values().any(|w| w.id == options.id) {
            return self.report(&format!("a window {} is open already", options.id));
        }
        let [r, g, b, a] = options.background;
        let (surface, shared, page) = if options.direct {
            // CEF's own window: Chromium draws and shows it (no wgpu, no embeds).
            let shared = Shared { state: Rc::new(RefCell::new(PageState { size: (options.width as i32, options.height as i32), scale: 1.0, ..Default::default() })), gpu: self.gpu.clone() };
            let argb = (((a * 255.0) as u32) << 24) | (((r * 255.0) as u32) << 16) | (((g * 255.0) as u32) << 8) | ((b * 255.0) as u32);
            let direct = browser::DirectWindow { title: options.title.clone().or_else(|| options.initial_title.clone()), width: options.width as i32, height: options.height as i32, app_id: options.app_id.clone() };
            let page = Page::open(&shared, &options.url, Some(direct), argb);
            (None, shared, page)
        } else {
            let mut attributes = Window::default_attributes().with_title(options.title.clone().or_else(|| options.initial_title.clone()).unwrap_or_else(|| "salir".into())).with_inner_size(winit::dpi::LogicalSize::new(options.width, options.height)).with_transparent(a < 1.0);
            #[cfg(target_os = "linux")]
            if let Some(app_id) = &options.app_id {
                use winit::platform::wayland::WindowAttributesExtWayland;
                attributes = WindowAttributesExtWayland::with_name(attributes, app_id, "");
            }
            let window = match event_loop.create_window(attributes) {
                Ok(w) => Arc::new(w),
                Err(e) => return self.report(&format!("window {}: {e}", options.id)),
            };
            let size = window.inner_size();
            let surface = match self.gpu.surface(window.clone(), size.width, size.height) {
                Ok(s) => s,
                Err(e) => return self.report(&format!("window {}: {e}", options.id)),
            };
            let scale = window.scale_factor();
            let logical = size.to_logical::<f64>(scale);
            let shared = Shared { state: Rc::new(RefCell::new(PageState { size: (logical.width as i32, logical.height as i32), scale: scale as f32, ..Default::default() })), gpu: self.gpu.clone() };
            let page = Page::open(&shared, &options.url, None, 0);
            (Some(surface), shared, page)
        };
        if page.is_none() {
            return self.report(&format!("window {}: no page", options.id));
        }
        let key = self.next_key;
        self.next_key += 1;
        self.windows.insert(
            key,
            Win {
                key,
                id: options.id.clone(),
                surface,
                page,
                shared,
                embeds: vec![],
                fixed_title: options.title.is_some(),
                background: wgpu::Color { r: r * a, g: g * a, b: b * a, a },
                cursor: (0.0, 0.0),
                buttons: 0,
                modifiers: ModifiersState::empty(),
                last_click: None,
                closing: false,
                capture: None,
                focused: true,
            },
        );
        trace!("window {} opened{}: {}", options.id, if options.direct { " (direct)" } else { "" }, options.url);
        if self.control {
            control::emit(json!({ "opened": options.id }));
        }
    }

    fn report(&self, error: &str) {
        if self.control {
            control::emit(json!({ "error": error }));
        } else {
            eprintln!("salir: {error}");
        }
    }

    fn win_by_id(&mut self, id: &str) -> Option<&mut Win> {
        self.windows.values_mut().find(|w| w.id == id)
    }

    fn command(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Open(options) => self.open(event_loop, options),
            Command::Trust(pem) => {
                if let Err(e) = browser::trust_certificate(&pem) {
                    self.report(&format!("trust_certificate: {e}"));
                }
            }
            Command::Close(id) => match self.win_by_id(&id) {
                Some(w) => close(w),
                None => self.report(&format!("no window {id}")),
            },
            Command::Send { window, message } => match self.win_by_id(&window) {
                Some(w) => {
                    if let Some(page) = &w.page {
                        page.deliver(&json!({ "$": "message", "message": message }));
                    }
                }
                None => self.report(&format!("no window {window}")),
            },
            Command::Quit => self.quit(),
        }
    }

    fn quit(&mut self) {
        self.quitting = true;
        for w in self.windows.values_mut() {
            close(w);
        }
    }

    // What the pages did since last looked at: what they said, their titles and cursors, closed.
    fn read_pages(&mut self) {
        let keys: Vec<u64> = self.windows.keys().copied().collect();
        for key in keys {
            let Some(w) = self.windows.get_mut(&key) else { continue };
            let (messages, title, cursor, popups, painted, closed, error) = {
                let mut s = w.shared.state.borrow_mut();
                (std::mem::take(&mut s.messages), s.title.take(), s.cursor.take(), std::mem::take(&mut s.popups), std::mem::replace(&mut s.painted, false), s.closed, s.error.take())
            };
            if let Some(title) = title.filter(|_| !w.fixed_title) {
                match w.window() {
                    Some(window) => window.set_title(&title),
                    None => {
                        if let Some(page) = &w.page {
                            page.set_title(&title);
                        }
                    }
                }
            }
            if let (Some(cursor), Some(window)) = (cursor, w.window())
                && w.capture.is_none_or(|(_, lock)| !lock)
            {
                window.set_cursor(cursor); // (not while it's locked and hidden)
            }
            if painted {
                w.redraw();
            }
            let id = w.id.clone();
            if let Some(e) = error {
                self.report(&format!("window {id}: a frame couldn't be shared on the GPU: {e}"));
            }
            for url in popups {
                if self.control {
                    control::emit(json!({ "popup": { "window": id, "url": url } }));
                }
            }
            for message in messages {
                self.page_message(key, message);
            }
            if closed {
                self.remove(key);
            }
        }
    }

    fn remove(&mut self, key: u64) {
        let Some(mut w) = self.windows.remove(&key) else { return };
        self.drop_embeds(&mut w, |_| true);
        w.page = None;
        if self.control {
            control::emit(json!({ "closed": w.id }));
        }
    }

    // A message from a page's window.salir.
    fn page_message(&mut self, key: u64, m: Value) {
        trace!("page {key}: {m}");
        let id = m.get("id").and_then(Value::as_u64).unwrap_or(0);
        match m.get("$").and_then(Value::as_str).unwrap_or("") {
            "hello" => {
                // A new page: the last one's embeds are gone with it.
                if let Some(mut w) = self.windows.remove(&key) {
                    w.release(false);
                    self.drop_embeds(&mut w, |_| true);
                    self.windows.insert(key, w);
                }
            }
            "message" => {
                let window = self.windows.get(&key).map(|w| w.id.clone()).unwrap_or_default();
                let message = m.get("message").cloned().unwrap_or(Value::Null);
                if let Some(f) = &mut self.on_message {
                    f(&window, message.clone());
                }
                if self.control {
                    control::emit(json!({ "message": { "window": window, "message": message } }));
                }
            }
            "close" => {
                // The page's window closed (salir.close()), as its close button would; off the
                // screen at once (the page's decided: nothing to ask), the page closing unseen.
                if let Some(w) = self.windows.get_mut(&key) {
                    if !w.closing {
                        w.surface = None;
                        trace!("window {key}: hidden, closing");
                    }
                    close(w);
                }
            }
            "embed.create" => {
                let kind = m.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
                let options = m.get("options").cloned().unwrap_or(Value::Null);
                if let Err(e) = self.create_embed(key, id, &kind, options)
                    && let Some(page) = self.windows.get(&key).and_then(|w| w.page.as_ref())
                {
                    page.deliver(&json!({ "$": "embed.error", "id": id, "error": e }));
                }
            }
            "embed.rect" => {
                if let Some(w) = self.windows.get_mut(&key)
                    && let Some(e) = w.embeds.iter_mut().find(|e| e.id == id)
                {
                    e.rect = m.get("rect").and_then(|r| serde_json::from_value(r.clone()).ok());
                    w.redraw();
                }
            }
            "embed.message" => {
                let message = m.get("message").cloned().unwrap_or(Value::Null);
                self.with_gl_current();
                if let Some(e) = self.windows.get_mut(&key).and_then(|w| w.embeds.iter_mut().find(|e| e.id == id)) {
                    match &mut e.imp {
                        Imp::Wgpu { embed, .. } => embed.message(message),
                        #[cfg(target_os = "linux")]
                        Imp::Gl { embed, .. } => embed.message(message),
                    }
                }
            }
            "embed.remove" => {
                if let Some(mut w) = self.windows.remove(&key) {
                    if w.capture.is_some_and(|(c, _)| c == id) {
                        w.release(false);
                    }
                    self.drop_embeds(&mut w, |e| e.id == id);
                    w.redraw();
                    self.windows.insert(key, w);
                }
            }
            "embed.input" => {
                // input="direct" (lock: the pointer locked too) set on an embed, or not any more.
                if let Some(w) = self.windows.get_mut(&key) {
                    let direct = m.get("mode").and_then(Value::as_str) == Some("direct");
                    let lock = m.get("lock").and_then(Value::as_bool).unwrap_or(false);
                    if direct && w.surface.is_some() && w.embeds.iter().any(|e| e.id == id) {
                        if w.capture.is_some_and(|(c, _)| c != id) {
                            w.release(true); // (another had it)
                        }
                        let was_locked = w.capture.is_some_and(|(_, l)| l);
                        w.capture = Some((id, lock));
                        if lock != was_locked && w.focused {
                            w.grab(lock);
                        }
                        trace!("window {}: input to embed {id} directly{}", w.id, if lock { ", the pointer locked" } else { "" });
                    } else if w.capture.is_some_and(|(c, _)| c == id) {
                        w.release(false);
                        trace!("window {}: input to the page again", w.id);
                    }
                }
            }
            _ => {}
        }
    }

    fn create_embed(&mut self, key: u64, id: u64, kind: &str, options: Value) -> Result<(), String> {
        let link = Link { window: key, embed: id, proxy: self.proxy.clone() };
        let imp = if let Some(factory) = self.embeds.get(kind) {
            Imp::Wgpu { embed: factory(options, link)?, target: None }
        } else if self.gl_embeds.contains_key(kind) {
            #[cfg(target_os = "linux")]
            {
                let gl = self.gl()?;
                gl.make_current();
                let lookup = |name: &str| gl.proc_address(name);
                Imp::Gl { embed: self.gl_embeds[kind](options, link, &lookup)?, buffers: vec![], next: 0, shown: None }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = link;
                return Err("GL embeds: Linux only, so far".into());
            }
        } else {
            return Err(format!("no embed \"{kind}\" (salir --plugin {kind}=path.so)"));
        };
        let w = self.windows.get_mut(&key).ok_or("no window")?;
        w.embeds.push(EmbedLayer { id, rect: None, wants_frame: true, imp });
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn gl(&mut self) -> Result<Rc<crate::gl::Gl>, String> {
        let pci = self.gpu.pci_bus();
        self.gl.get_or_insert_with(|| crate::gl::Gl::new(&pci).map(Rc::new)).clone()
    }

    fn with_gl_current(&self) {
        #[cfg(target_os = "linux")]
        if let Some(Ok(gl)) = &self.gl {
            gl.make_current();
        }
    }

    // A window's embeds that `which`, gone: torn down later (bury).
    fn drop_embeds(&mut self, w: &mut Win, which: impl Fn(&EmbedLayer) -> bool) {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut w.embeds).into_iter().partition(|e| which(e));
        w.embeds = kept;
        let turn = self.turn;
        self.dying.extend(gone.into_iter().map(|e| (turn, e)));
    }

    // One gone embed torn down (GL's with its context current, its buffers freed): one that went
    // before this turn, or (`all`, quitting) every one.
    fn bury(&mut self, all: bool) {
        while let Some((turn, _)) = self.dying.front() {
            if !all && *turn >= self.turn {
                return;
            }
            let Some((_, e)) = self.dying.pop_front() else { return };
            self.with_gl_current();
            match e.imp {
                Imp::Wgpu { embed, .. } => drop(embed),
                #[cfg(target_os = "linux")]
                Imp::Gl { embed, buffers, .. } => {
                    drop(embed);
                    if let Some(Ok(gl)) = &self.gl {
                        for (mut b, _) in buffers {
                            gl.drop_buffer(&mut b);
                        }
                    }
                }
            }
            if !all {
                return;
            }
        }
    }

    // A window's frame: its embeds drawn (those that asked), then put together with the page.
    fn draw(&mut self, key: u64) {
        trace!("drawing window {key}");
        #[cfg(target_os = "linux")]
        let gl = match &self.gl {
            Some(Ok(gl)) => Some(gl.clone()),
            _ => None,
        };
        let gpu = self.gpu.clone();
        let Some(w) = self.windows.get_mut(&key) else { return };
        let Some(surface) = &w.surface else { return };
        let (ww, wh) = (surface.config.width, surface.config.height);
        let scale = w.shared.state.borrow().scale;
        for e in &mut w.embeds {
            let px = e.rect.map(|[x, y, rw, rh]| [x * scale, y * scale, rw * scale, rh * scale]);
            let size = px.map(|r| ((r[2].round() as u32).max(1), (r[3].round() as u32).max(1)));
            match &mut e.imp {
                Imp::Wgpu { embed, target } => {
                    let Some((width, height)) = size else {
                        if let Some((_, l)) = target {
                            l.put(&gpu.queue, None, (ww, wh), false);
                        }
                        continue;
                    };
                    if target.as_ref().is_none_or(|(t, _)| (t.width(), t.height()) != (width, height)) {
                        let texture = gpu.target(width, height, wgpu::TextureFormat::Rgba8Unorm);
                        let layer = gpu.layer(&texture.create_view(&Default::default()));
                        *target = Some((texture, layer));
                        e.wants_frame = true;
                    }
                    let (texture, layer) = target.as_mut().unwrap();
                    if std::mem::take(&mut e.wants_frame) {
                        let view = texture.create_view(&Default::default());
                        embed.render(&Frame { device: &gpu.device, queue: &gpu.queue, target: &view, format: texture.format(), width, height, scale });
                    }
                    layer.put(&gpu.queue, px, (ww, wh), false);
                }
                #[cfg(target_os = "linux")]
                Imp::Gl { embed, buffers, next, shown } => {
                    let Some(gl) = &gl else { continue };
                    let Some((width, height)) = size else {
                        for (_, l) in buffers.iter_mut() {
                            l.put(&gpu.queue, None, (ww, wh), true);
                        }
                        continue;
                    };
                    if buffers.first().is_none_or(|(b, _)| (b.width, b.height) != (width, height)) {
                        for (mut b, _) in buffers.drain(..) {
                            gl.drop_buffer(&mut b);
                        }
                        for _ in 0..GL_BUFFERS {
                            match gl.buffer(&gpu.device, width, height) {
                                Ok(b) => {
                                    let layer = gpu.layer(&b.wgpu.as_ref().unwrap().create_view(&Default::default()));
                                    buffers.push((b, layer));
                                }
                                Err(e) => {
                                    eprintln!("salir: an embed's buffer: {e}");
                                    break;
                                }
                            }
                        }
                        (*next, *shown) = (0, None);
                        e.wants_frame = true;
                    }
                    if buffers.is_empty() {
                        continue;
                    }
                    if std::mem::take(&mut e.wants_frame) {
                        let i = *next % buffers.len();
                        gl.bind(&buffers[i].0);
                        embed.render(&GlFrame { fbo: buffers[i].0.fbo, width, height, scale });
                        gl.finish();
                        (*shown, *next) = (Some(i), i + 1);
                    }
                    for (i, (_, l)) in buffers.iter_mut().enumerate() {
                        l.put(&gpu.queue, if Some(i) == *shown { px } else { None }, (ww, wh), true);
                    }
                }
            }
        }
        // The page, and its popup.
        let mut state = w.shared.state.borrow_mut();
        let popup_rect = state.popup_rect.map(|[x, y, pw, ph]| [x as f32 * scale, y as f32 * scale, pw as f32 * scale, ph as f32 * scale]);
        if let Some((t, l)) = &mut state.page {
            let at = [0.0, 0.0, t.width() as f32, t.height() as f32];
            l.put(&gpu.queue, Some(at), (ww, wh), false);
        }
        if let Some((t, l)) = &mut state.popup {
            let at = popup_rect.map(|[x, y, _, _]| [x, y, t.width() as f32, t.height() as f32]);
            l.put(&gpu.queue, at, (ww, wh), false);
        }
        let mut layers: Vec<&Layer> = vec![];
        for e in &w.embeds {
            match &e.imp {
                Imp::Wgpu { target, .. } => layers.extend(target.as_ref().map(|(_, l)| l)),
                #[cfg(target_os = "linux")]
                Imp::Gl { buffers, .. } => layers.extend(buffers.iter().map(|(_, l)| l)),
            }
        }
        layers.extend(state.page.as_ref().map(|(_, l)| l));
        layers.extend(state.popup.as_ref().map(|(_, l)| l));
        if let Some(surface) = &w.surface {
            gpu.compose(surface, w.background, &layers);
        }
    }
}

fn close(w: &mut Win) {
    if w.closing {
        return;
    }
    w.closing = true;
    match &w.page {
        Some(page) => page.close(),
        None => w.shared.state.borrow_mut().closed = true,
    }
}

// A window event as an embed's direct input (it has the window's input: input="direct").
// True: it was input, and the embed's (not the page's).
fn direct_input(w: &mut Win, event: &WindowEvent) -> bool {
    use crate::Input;
    let scale = w.window().map_or(1.0, |win| win.scale_factor());
    let (ox, oy) = w.capture_origin();
    let input = match event {
        WindowEvent::CursorMoved { position, .. } => {
            let p = position.to_logical::<f64>(scale);
            w.cursor = (p.x, p.y);
            Input::Move { x: p.x as f32 - ox, y: p.y as f32 - oy }
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let button = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                MouseButton::Back => 3,
                MouseButton::Forward => 4,
                MouseButton::Other(n) => (*n).min(255) as u8,
            };
            Input::Button { button, down: *state == ElementState::Pressed, x: w.cursor.0 as f32 - ox, y: w.cursor.1 as f32 - oy }
        }
        WindowEvent::MouseWheel { delta, .. } => match delta {
            MouseScrollDelta::LineDelta(x, y) => Input::Wheel { dx: *x, dy: *y, lines: true },
            MouseScrollDelta::PixelDelta(p) => {
                let p = p.to_logical::<f64>(scale);
                Input::Wheel { dx: p.x as f32, dy: p.y as f32, lines: false }
            }
        },
        WindowEvent::KeyboardInput { event, .. } => {
            let code = match event.physical_key {
                winit::keyboard::PhysicalKey::Code(c) => format!("{c:?}"),
                _ => String::new(),
            };
            let key = match &event.logical_key {
                Key::Character(c) => c.to_string(),
                Key::Named(n) => format!("{n:?}"),
                _ => String::new(),
            };
            Input::Key { code, key, down: event.state == ElementState::Pressed, repeat: event.repeat, text: event.text.as_ref().map(|t| t.to_string()) }
        }
        WindowEvent::ModifiersChanged(m) => {
            w.modifiers = m.state();
            return false; // (the page's to know too, for when it has the input again)
        }
        _ => return false,
    };
    w.capture_input(&input);
    true
}

fn modifier_flags(m: ModifiersState) -> u32 {
    let mut f = 0;
    if m.shift_key() {
        f |= flags::SHIFT;
    }
    if m.control_key() {
        f |= flags::CONTROL;
    }
    if m.alt_key() {
        f |= flags::ALT;
    }
    if m.super_key() {
        f |= flags::META;
    }
    f
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        trace!("resumed: {} windows to open", self.pending.len());
        for options in std::mem::take(&mut self.pending) {
            self.open(event_loop, options);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Pump => {}
            Event::EmbedSend { window, embed, message } => {
                if let Some(page) = self.windows.get(&window).and_then(|w| w.page.as_ref()) {
                    page.deliver(&json!({ "$": "embed.message", "id": embed, "message": message }));
                }
            }
            Event::EmbedFrame { window, embed } => {
                if let Some(w) = self.windows.get_mut(&window)
                    && let Some(e) = w.embeds.iter_mut().find(|e| e.id == embed)
                {
                    e.wants_frame = true;
                    w.redraw();
                }
            }
            Event::EmbedRelease { window, embed } => {
                if let Some(w) = self.windows.get_mut(&window)
                    && w.capture.is_some_and(|(c, _)| c == embed)
                {
                    w.release(true);
                }
            }
            Event::Control(command) => self.command(event_loop, command),
            Event::ControlClosed => self.quit(),
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(key) = self.windows.values().find(|w| w.window().is_some_and(|win| win.id() == id)).map(|w| w.key) else { return };
        if let WindowEvent::RedrawRequested = event {
            return self.draw(key);
        }
        let gpu = self.gpu.clone();
        let Some(w) = self.windows.get_mut(&key) else { return };
        if let WindowEvent::Focused(focused) = event {
            w.focused = focused;
            if let Some((_, lock)) = w.capture {
                // (Locked only while it has the keyboard: elsewhere the pointer's the desktop's.)
                if lock {
                    w.grab(focused);
                }
                w.capture_input(&crate::Input::Focus(focused));
            }
        }
        if w.capture.is_some() && direct_input(w, &event) {
            return;
        }
        let Some(page) = &w.page else { return };
        let Some(window) = w.window().cloned() else { return };
        match event {
            WindowEvent::CloseRequested => close(w),
            WindowEvent::Resized(size) => {
                trace!("window {} resized: {}×{} px", w.id, size.width, size.height);
                if let Some(surface) = &mut w.surface {
                    surface.resize(&gpu, size.width, size.height);
                }
                let logical = size.to_logical::<f64>(window.scale_factor());
                {
                    let mut s = w.shared.state.borrow_mut();
                    s.size = (logical.width as i32, logical.height as i32);
                    s.resized();
                }
                page.resized();
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                w.shared.state.borrow_mut().scale = scale_factor as f32;
                page.screen_changed();
            }
            WindowEvent::Focused(focused) => page.focus(focused),
            WindowEvent::ModifiersChanged(m) => w.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f64>(window.scale_factor());
                w.cursor = (p.x, p.y);
                page.mouse_move(p.x as i32, p.y as i32, modifier_flags(w.modifiers) | w.buttons, false);
            }
            WindowEvent::CursorLeft { .. } => page.mouse_move(w.cursor.0 as i32, w.cursor.1 as i32, modifier_flags(w.modifiers) | w.buttons, true),
            WindowEvent::MouseInput { state, button, .. } => {
                let (b, flag) = match button {
                    MouseButton::Left => (0, flags::LEFT),
                    MouseButton::Middle => (1, flags::MIDDLE),
                    MouseButton::Right => (2, flags::RIGHT),
                    _ => return,
                };
                let down = state == ElementState::Pressed;
                // Double (triple...) clicks: the same button again soon, near the last.
                let clicks = if down {
                    let n = match w.last_click {
                        Some((at, lb, (x, y), n)) if lb == b && at.elapsed() < Duration::from_millis(500) && (x - w.cursor.0).abs() < 4.0 && (y - w.cursor.1).abs() < 4.0 => n + 1,
                        _ => 1,
                    };
                    w.last_click = Some((Instant::now(), b, w.cursor, n));
                    w.buttons |= flag;
                    n
                } else {
                    w.buttons &= !flag;
                    w.last_click.map_or(1, |c| c.3)
                };
                page.mouse_button(w.cursor.0 as i32, w.cursor.1 as i32, modifier_flags(w.modifiers) | w.buttons, b, !down, clicks);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // A notch: 120 (Windows' WHEEL_DELTA, what Chromium counts in); a touchpad: its pixels.
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        let p = p.to_logical::<f64>(window.scale_factor());
                        (p.x as f32, p.y as f32)
                    }
                };
                page.wheel(w.cursor.0 as i32, w.cursor.1 as i32, modifier_flags(w.modifiers) | w.buttons, dx as i32, dy as i32);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let f = modifier_flags(w.modifiers);
                let code = keys::windows_key_code(&event.logical_key, event.physical_key);
                let native = keys::native_key_code(event.physical_key);
                let down = event.state == ElementState::Pressed;
                trace!("window {key}: key {:?} {}", event.logical_key, if down { "down" } else { "up" });
                page.key(down, code, native, f);
                if !down {
                    return;
                }
                if event.logical_key == Key::Named(NamedKey::Enter) {
                    page.char(0x0d, 0x0d, f);
                } else if let Some(text) = event.text.filter(|t| !w.modifiers.control_key() && !w.modifiers.alt_key() && !w.modifiers.super_key() && t.chars().all(|c| !c.is_control())) {
                    page.text(&text);
                }
            }
            _ => {}
        }
    }

    // The mouse's own motion (raw): to an embed with the pointer locked, in the focused window.
    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _device: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && let Some(w) = self.windows.values_mut().find(|w| w.focused && w.capture.is_some_and(|(_, lock)| lock))
        {
            w.capture_input(&crate::Input::Motion { dx, dy });
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.turn += 1;
        let now = Instant::now();
        // CEF's work: when it asked for it, and at least every 1/30 s whether it did or not.
        let asked = PUMP_AT.lock().unwrap().is_some_and(|at| at <= now);
        let last = *PUMPED.lock().unwrap();
        let interval = if self.windows.values().any(|w| w.surface.is_none()) { PUMP_DIRECT } else { PUMP_AT_LEAST };
        if asked || last.is_none_or(|l| now - l >= interval) {
            *PUMP_AT.lock().unwrap() = None;
            *PUMPED.lock().unwrap() = Some(now);
            browser::work();
        }
        self.read_pages();
        if self.windows.is_empty() && (!self.control || self.quitting) && self.pending.is_empty() {
            self.bury(true);
            return event_loop.exit();
        }
        self.bury(false);
        let mut next = PUMP_AT.lock().unwrap().unwrap_or(now + interval).min(PUMPED.lock().unwrap().unwrap_or(now) + interval);
        if !self.dying.is_empty() {
            next = Instant::now(); // (the next at once, after a look at what's come)
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }
}

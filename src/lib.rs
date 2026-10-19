//! salir: web pages over native GPU content. Each window is drawn with wgpu; its
//! page by CEF (Chromium) off screen, each frame handed over as a GPU texture (no
//! copy through memory); under the page, embeds: native content where an element
//! of the page is (salir.embed() in the page), drawn with wgpu or OpenGL, in this
//! program (Salir::embed, Salir::gl_embed) or a plugin (Salir::plugin, include/salir.h).
//!
//! ```no_run
//! fn main() -> std::process::ExitCode {
//!     salir::Salir::new("my-app")
//!         .window(salir::WindowOptions::new("main", "https://example.com"))
//!         .run()
//! }
//! ```
//!
//! The program is CEF's other processes too (pages', the GPU's): run() is them
//! when it's one, so keep what comes before it in main() light.

#[macro_use]
mod app;
mod browser;
mod control;
mod embed;
mod gpu;
mod keys;
mod plugin;
#[cfg(target_os = "linux")]
mod dmabuf;
#[cfg(target_os = "linux")]
mod gl;

pub use embed::{Embed, Frame, GlEmbed, GlFrame, Input, Link, ProcAddress};
pub use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// A window: a page at `url`; `id`: what --control calls it.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub id: String,
    pub url: String,
    /// Fixed; None: the page's.
    pub title: Option<String>,
    /// Its title till the page has one (window rules match it: a window told apart from the
    /// others as it opens). None: "salir".
    pub initial_title: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Its Wayland app id (window rules); None: the program's.
    pub app_id: Option<String>,
    /// What's under the page and its embeds: red, green, blue, alpha (0 to 1; under 1: a see-through window).
    pub background: [f64; 4],
    /// The page in a window of its own, drawn and shown by Chromium: no compositing (no extra
    /// frame of latency), no embeds. For pages that are all there is: a game in WebGPU, an app's UI.
    pub direct: bool,
    /// Its page may have local files as File objects (salir.files(paths)): to drag them out to
    /// other programs, say. Its own page alone (the top frame, the window's origin): it gets to
    /// read them, so only for an app's own pages, never a web page.
    pub files: bool,
}

impl WindowOptions {
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> WindowOptions {
        WindowOptions { id: id.into(), url: url.into(), title: None, initial_title: None, width: 1280, height: 800, app_id: None, background: [0.0, 0.0, 0.0, 1.0], direct: false, files: false }
    }
}

/// An app: its windows, embeds and plugins; run() runs it.
pub struct Salir {
    name: String,
    data: Option<PathBuf>,
    windows: Vec<WindowOptions>,
    embeds: HashMap<String, embed::EmbedFactory>,
    gl_embeds: HashMap<String, embed::GlEmbedFactory>,
    plugins: Vec<(String, PathBuf)>,
    control: bool,
    on_message: Option<OnMessage>,
}

type OnMessage = Box<dyn FnMut(&str, Value)>;

impl Salir {
    /// `name`: its data's folder's (~/.cache/<name>: CEF's cache, cookies, storage).
    pub fn new(name: impl Into<String>) -> Salir {
        Salir { name: name.into(), data: None, windows: vec![], embeds: HashMap::new(), gl_embeds: HashMap::new(), plugins: vec![], control: false, on_message: None }
    }

    /// Its data (CEF's profile: cache, cookies, storage) in `dir`, not ~/.cache/<name>.
    pub fn data(mut self, dir: impl Into<PathBuf>) -> Salir {
        self.data = Some(dir.into());
        self
    }

    /// `pem` (a certificate) trusted whatever its errors: an app's own server's, made for the
    /// run (HTTPS on 127.0.0.1, for HTTP/2). Exactly it: any other's errors refuse it, as ever.
    pub fn trust_certificate(self, pem: &str) -> Salir {
        if let Err(e) = browser::trust_certificate(pem) {
            eprintln!("salir: a certificate to trust: {e}");
        }
        self
    }

    pub fn window(mut self, options: WindowOptions) -> Salir {
        self.windows.push(options);
        self
    }

    /// Embeds of `kind` drawn with wgpu: made by `make(options, link)` when a page asks for one.
    pub fn embed(mut self, kind: &str, make: impl Fn(Value, Link) -> Result<Box<dyn Embed>, String> + 'static) -> Salir {
        self.embeds.insert(kind.into(), Box::new(make));
        self
    }

    /// Embeds of `kind` drawn with OpenGL (libmpv's render API and the like): `make(options, link,
    /// get_proc_address)`, salir's GL context current.
    pub fn gl_embed(mut self, kind: &str, make: impl Fn(Value, Link, ProcAddress) -> Result<Box<dyn GlEmbed>, String> + 'static) -> Salir {
        self.gl_embeds.insert(kind.into(), Box::new(make));
        self
    }

    /// Embeds of `kind` from a plugin (a shared library: include/salir.h).
    pub fn plugin(mut self, kind: &str, path: impl Into<PathBuf>) -> Salir {
        self.plugins.push((kind.into(), path.into()));
        self
    }

    /// Driven through stdin and stdout (JSON lines: control.rs): windows opened and closed,
    /// messages to and from pages; it runs till told to quit, or stdin closes.
    pub fn control(mut self, on: bool) -> Salir {
        self.control = on;
        self
    }

    /// What pages send (salir.send(message)): `f(window id, message)`.
    pub fn on_message(mut self, f: impl FnMut(&str, Value) + 'static) -> Salir {
        self.on_message = Some(Box::new(f));
        self
    }

    /// Runs it (or, in one of CEF's other processes, that), till its windows are closed.
    pub fn run(self) -> std::process::ExitCode {
        app::run(self)
    }
}

//! Embeds: native content in a page, drawn under it where an element of it is
//! (salir.embed(element, kind, options) in the page): wgpu's (Embed), OpenGL's
//! (GlEmbed: libmpv's render API and the like), or a plugin's (plugin.rs, GL in C).
//! The page draws over it (its controls), sees through to it where it's clear,
//! and passes it what it's told (handle.send()), and the input on the element if
//! it asked for it.

use serde_json::Value;
use std::ffi::c_void;
use winit::event_loop::EventLoopProxy;

/// What an embed draws with wgpu: `target`, `width`×`height` pixels (kept between frames).
pub struct Frame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    /// Pixels to a CSS pixel (the page's).
    pub scale: f32,
}

/// Native content drawn with wgpu.
pub trait Embed {
    /// Draw: when it's made, resized, or after link.request_frame().
    fn render(&mut self, frame: &Frame);
    /// A message from the page (handle.send(message); input the page passed on, as {"input": {...}}).
    fn message(&mut self, _message: Value) {}
    /// The window's input, straight from it (the element's input="direct"): not through the page.
    fn input(&mut self, _input: &Input) {}
}

/// The window's input, given to an embed directly (input="direct"). Places: CSS pixels
/// from the embed's top left.
#[derive(Clone, Debug)]
pub enum Input {
    /// The pointer moved to (x, y).
    Move { x: f32, y: f32 },
    /// The mouse moved by (dx, dy), its own units (raw: locked or not, at the screen's edge too).
    Motion { dx: f64, dy: f64 },
    /// `button`: 0 left, 1 middle, 2 right, 3 back, 4 forward.
    Button { button: u8, down: bool, x: f32, y: f32 },
    /// Scrolled: in lines (a mouse's wheel), or pixels (a touchpad's).
    Wheel { dx: f32, dy: f32, lines: bool },
    /// `code`: where it is (DOM's names: "KeyW", "Space", "ArrowUp"); `key`: what it is in the
    /// layout ("w", "Enter"); `text`: what it types, if anything.
    Key { code: String, key: String, down: bool, repeat: bool, text: Option<String> },
    /// The window got (true) or lost the keyboard.
    Focus(bool),
}

impl Input {
    /// As JSON ({"type": "move", ...}): what a plugin is given ({"input": this}).
    pub fn to_json(&self) -> Value {
        use serde_json::json;
        match self {
            Input::Move { x, y } => json!({ "type": "move", "x": x, "y": y, "direct": true }),
            Input::Motion { dx, dy } => json!({ "type": "motion", "dx": dx, "dy": dy, "direct": true }),
            Input::Button { button, down, x, y } => json!({ "type": if *down { "down" } else { "up" }, "button": button, "x": x, "y": y, "direct": true }),
            Input::Wheel { dx, dy, lines } => json!({ "type": "wheel", "dx": dx, "dy": dy, "lines": lines, "direct": true }),
            Input::Key { code, key, down, repeat, text } => json!({ "type": if *down { "keydown" } else { "keyup" }, "code": code, "key": key, "repeat": repeat, "text": text, "direct": true }),
            Input::Focus(f) => json!({ "type": if *f { "focus" } else { "blur" }, "direct": true }),
        }
    }
}

/// What an embed draws with OpenGL: `fbo`, `width`×`height` pixels, as GL draws (y up).
pub struct GlFrame {
    pub fbo: u32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// Native content drawn with OpenGL, in salir's context (current in every call). Linux only.
pub trait GlEmbed {
    fn render(&mut self, frame: &GlFrame);
    fn message(&mut self, _message: Value) {}
    fn input(&mut self, _input: &Input) {}
}

/// GL's (and EGL's) functions, by name, for a GlEmbed to load.
pub type ProcAddress<'a> = &'a dyn Fn(&str) -> *const c_void;

pub type EmbedFactory = Box<dyn Fn(Value, Link) -> Result<Box<dyn Embed>, String>>;
pub type GlEmbedFactory = Box<dyn Fn(Value, Link, ProcAddress) -> Result<Box<dyn GlEmbed>, String>>;

/// An embed's way back: to its page, and to ask for a frame. Any thread.
#[derive(Clone)]
pub struct Link {
    pub(crate) window: u64,
    pub(crate) embed: u64,
    pub(crate) proxy: EventLoopProxy<crate::app::Event>,
}

impl Link {
    /// To the page: the embed's handle's on(fn).
    pub fn send(&self, message: Value) {
        let _ = self.proxy.send_event(crate::app::Event::EmbedSend { window: self.window, embed: self.embed, message });
    }

    /// render() again, at the next frame (once however often it's asked for before then).
    pub fn request_frame(&self) {
        let _ = self.proxy.send_event(crate::app::Event::EmbedFrame { window: self.window, embed: self.embed });
    }

    /// The window's input back to the page (input="direct" no more: its element's input
    /// attribute removed, a "release" event on it). Escape's, say.
    pub fn release_input(&self) {
        let _ = self.proxy.send_event(crate::app::Event::EmbedRelease { window: self.window, embed: self.embed });
    }
}

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
    /// A message from the page (handle.send(message); its input as {"input": {...}}).
    fn message(&mut self, _message: Value) {}
}

/// What an embed draws with OpenGL: `fbo`, `width`×`height` pixels, as GL draws (y up).
pub struct GlFrame {
    pub fbo: u32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// Native content drawn with OpenGL, in salir's context (current in every call).
pub trait GlEmbed {
    fn render(&mut self, frame: &GlFrame);
    fn message(&mut self, _message: Value) {}
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
}

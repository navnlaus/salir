//! Plugins: GL embeds in a shared library, any language, through include/salir.h.

use crate::embed::{GlEmbed, GlEmbedFactory, GlFrame, Link, ProcAddress};
use serde_json::Value;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::sync::Arc;

pub const ABI: u32 = 1;

#[repr(C)]
struct Host {
    abi: u32,
    ctx: *mut c_void,
    get_proc_address: extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    send: extern "C" fn(*mut c_void, *const c_char),
    request_frame: extern "C" fn(*mut c_void),
}

#[repr(C)]
struct Api {
    abi: u32,
    create: unsafe extern "C" fn(*const Host, *const c_char) -> *mut c_void,
    render: unsafe extern "C" fn(*mut c_void, u32, i32, i32),
    message: unsafe extern "C" fn(*mut c_void, *const c_char),
    destroy: unsafe extern "C" fn(*mut c_void),
}

// What the host's calls reach: the embed's link, and GL's functions.
struct Ctx {
    link: Link,
    proc_address: Box<dyn Fn(&str) -> *const c_void>,
}

extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let ctx = unsafe { &*(ctx as *const Ctx) };
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    (ctx.proc_address)(&name) as *mut c_void
}

extern "C" fn send(ctx: *mut c_void, json: *const c_char) {
    let ctx = unsafe { &*(ctx as *const Ctx) };
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy();
    ctx.link.send(serde_json::from_str(&text).unwrap_or(Value::String(text.into_owned())));
}

extern "C" fn request_frame(ctx: *mut c_void) {
    unsafe { &*(ctx as *const Ctx) }.link.request_frame();
}

/// The plugin at `path`, as a factory of its embeds.
pub fn load(path: &Path) -> Result<GlEmbedFactory, String> {
    let lib = Arc::new(unsafe { libloading::Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))?);
    let api: *const Api = unsafe {
        let entry: libloading::Symbol<unsafe extern "C" fn() -> *const Api> = lib.get(b"salir_plugin_v1").map_err(|e| format!("{}: {e}", path.display()))?;
        entry()
    };
    if api.is_null() || unsafe { (*api).abi } != ABI {
        return Err(format!("{}: not salir's ABI {ABI}", path.display()));
    }
    Ok(Box::new(move |options: Value, link: Link, _: ProcAddress| {
        // (GL's lookup is the app's, there as long as it is: the factory's lives only for the call.)
        let ctx = Box::new(Ctx { link, proc_address: Box::new(crate::app::gl_proc_address) });
        let host = Box::new(Host { abi: ABI, ctx: &*ctx as *const Ctx as *mut c_void, get_proc_address, send, request_frame });
        let options = CString::new(options.to_string()).unwrap_or_default();
        let instance = unsafe { ((*api).create)(&*host, options.as_ptr()) };
        if instance.is_null() {
            return Err("the plugin couldn't make one".into());
        }
        Ok(Box::new(PluginEmbed { _lib: lib.clone(), api, instance, _host: host, _ctx: ctx }) as Box<dyn GlEmbed>)
    }))
}

struct PluginEmbed {
    _lib: Arc<libloading::Library>, // (loaded while one of its embeds is)
    api: *const Api,
    instance: *mut c_void,
    _host: Box<Host>,
    _ctx: Box<Ctx>,
}

impl GlEmbed for PluginEmbed {
    fn render(&mut self, frame: &GlFrame) {
        unsafe { ((*self.api).render)(self.instance, frame.fbo, frame.width as i32, frame.height as i32) }
    }

    fn message(&mut self, message: Value) {
        let json = CString::new(message.to_string()).unwrap_or_default();
        unsafe { ((*self.api).message)(self.instance, json.as_ptr()) }
    }
}

impl Drop for PluginEmbed {
    fn drop(&mut self) {
        unsafe { ((*self.api).destroy)(self.instance) }
    }
}

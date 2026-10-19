//! OpenGL for embeds that draw with it (libmpv's render API, most of what's out
//! there): one EGL context, with no window, on the GPU salir draws with; and
//! buffers it draws into that wgpu then samples, shared as dmabufs (exported by
//! EGL, imported by dmabuf.rs): no copy.

use crate::dmabuf::{self, Dmabuf};
use khronos_egl as egl;
use std::ffi::c_void;

type Egl = egl::DynamicInstance<egl::EGL1_5>;

// libEGL, loaded once (GL's functions are looked up through it from any thread: plugins').
static EGL: std::sync::OnceLock<Option<Egl>> = std::sync::OnceLock::new();

fn egl() -> Option<&'static Egl> {
    EGL.get_or_init(|| unsafe { Egl::load_required() }.ok()).as_ref()
}

/// GL's and EGL's functions, by name (null: none).
pub fn proc_address(name: &str) -> *const c_void {
    egl().and_then(|e| e.get_proc_address(name)).map_or(std::ptr::null(), |f| f as *const c_void)
}

// The GL functions salir itself uses (an embed gets every one by get_proc_address).
#[allow(non_snake_case)]
struct Fns {
    GenTextures: unsafe extern "C" fn(i32, *mut u32),
    DeleteTextures: unsafe extern "C" fn(i32, *const u32),
    BindTexture: unsafe extern "C" fn(u32, u32),
    TexStorage2D: unsafe extern "C" fn(u32, i32, u32, i32, i32),
    GenFramebuffers: unsafe extern "C" fn(i32, *mut u32),
    DeleteFramebuffers: unsafe extern "C" fn(i32, *const u32),
    BindFramebuffer: unsafe extern "C" fn(u32, u32),
    FramebufferTexture2D: unsafe extern "C" fn(u32, u32, u32, u32, i32),
    CheckFramebufferStatus: unsafe extern "C" fn(u32) -> u32,
    Viewport: unsafe extern "C" fn(i32, i32, i32, i32),
    Finish: unsafe extern "C" fn(),
    ExportQuery: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut i32, *mut i32, *mut u64) -> u32,
    Export: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut i32, *mut i32, *mut i32) -> u32,
}

const TEXTURE_2D: u32 = 0x0DE1;
const RGBA8: u32 = 0x8058;
const FRAMEBUFFER: u32 = 0x8D40;
const COLOR_ATTACHMENT0: u32 = 0x8CE0;
const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const EGL_GL_TEXTURE_2D: egl::Enum = 0x30B1;
const EGL_PLATFORM_DEVICE_EXT: egl::Enum = 0x313F;
const EGL_DRM_DEVICE_FILE_EXT: egl::Int = 0x3233;

pub struct Gl {
    egl: &'static Egl,
    display: egl::Display,
    context: egl::Context,
    fns: Fns,
}

impl Gl {
    /// GL on the GPU whose PCI bus id is `pci_bus` (wgpu's adapter's), if EGL can say; else the first.
    pub fn new(pci_bus: &str) -> Result<Gl, String> {
        let egl = egl().ok_or("no libEGL")?;
        let display = device_display(egl, pci_bus).or_else(|| unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }).ok_or("no EGL display")?;
        egl.initialize(display).map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_API).map_err(|e| format!("eglBindAPI: {e}"))?;
        // (No window: a pbuffer-capable config, else any. EGL's default asks for window ones.)
        let config = [egl::PBUFFER_BIT, egl::DONT_CARE]
            .into_iter()
            .find_map(|surface| egl.choose_first_config(display, &[egl::RENDERABLE_TYPE, egl::OPENGL_BIT, egl::SURFACE_TYPE, surface, egl::NONE]).ok().flatten())
            .ok_or("no EGL config for OpenGL")?;
        let core = [egl::CONTEXT_MAJOR_VERSION, 3, egl::CONTEXT_MINOR_VERSION, 3, egl::CONTEXT_OPENGL_PROFILE_MASK, egl::CONTEXT_OPENGL_CORE_PROFILE_BIT, egl::NONE];
        let context = egl.create_context(display, config, None, &core).or_else(|_| egl.create_context(display, config, None, &[egl::NONE])).map_err(|e| format!("eglCreateContext: {e}"))?;
        egl.make_current(display, None, None, Some(context)).map_err(|e| format!("eglMakeCurrent (no surface): {e}"))?;
        let get = |name: &str| egl.get_proc_address(name).map(|f| f as *const c_void).filter(|p| !p.is_null()).ok_or_else(|| format!("no {name}"));
        let fns = unsafe {
            Fns {
                GenTextures: cast(get("glGenTextures")?),
                DeleteTextures: cast(get("glDeleteTextures")?),
                BindTexture: cast(get("glBindTexture")?),
                TexStorage2D: cast(get("glTexStorage2D")?),
                GenFramebuffers: cast(get("glGenFramebuffers")?),
                DeleteFramebuffers: cast(get("glDeleteFramebuffers")?),
                BindFramebuffer: cast(get("glBindFramebuffer")?),
                FramebufferTexture2D: cast(get("glFramebufferTexture2D")?),
                CheckFramebufferStatus: cast(get("glCheckFramebufferStatus")?),
                Viewport: cast(get("glViewport")?),
                Finish: cast(get("glFinish")?),
                ExportQuery: cast(get("eglExportDMABUFImageQueryMESA")?),
                Export: cast(get("eglExportDMABUFImageMESA")?),
            }
        };
        Ok(Gl { egl, display, context, fns })
    }

    /// Makes salir's context the current one (an embed's calls come after this).
    pub fn make_current(&self) {
        let _ = self.egl.make_current(self.display, None, None, Some(self.context));
    }

    /// GL's and EGL's functions, by name.
    pub fn proc_address(&self, name: &str) -> *const c_void {
        self.egl.get_proc_address(name).map_or(std::ptr::null(), |f| f as *const c_void)
    }

    /// Waits for what's been drawn (before wgpu reads it).
    pub fn finish(&self) {
        unsafe { (self.fns.Finish)() }
    }

    /// A buffer `width`×`height` to draw into with GL (its framebuffer), and to sample with wgpu.
    pub fn buffer(&self, device: &wgpu::Device, width: u32, height: u32) -> Result<Buffer, String> {
        self.make_current();
        let f = &self.fns;
        unsafe {
            let mut texture = 0;
            (f.GenTextures)(1, &mut texture);
            (f.BindTexture)(TEXTURE_2D, texture);
            (f.TexStorage2D)(TEXTURE_2D, 1, RGBA8, width as i32, height as i32);
            (f.BindTexture)(TEXTURE_2D, 0);
            let mut fbo = 0;
            (f.GenFramebuffers)(1, &mut fbo);
            (f.BindFramebuffer)(FRAMEBUFFER, fbo);
            (f.FramebufferTexture2D)(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, texture, 0);
            let status = (f.CheckFramebufferStatus)(FRAMEBUFFER);
            (f.BindFramebuffer)(FRAMEBUFFER, 0);
            let mut buffer = Buffer { texture, fbo, image: None, wgpu: None, width, height };
            if status != FRAMEBUFFER_COMPLETE {
                self.drop_buffer(&mut buffer);
                return Err(format!("framebuffer incomplete ({status:#x})"));
            }
            // Shared: an EGLImage of the texture, exported as a dmabuf, imported into wgpu.
            let image = self
                .egl
                .create_image(self.display, egl::Context::from_ptr(self.context.as_ptr()), EGL_GL_TEXTURE_2D, egl::ClientBuffer::from_ptr(texture as usize as *mut c_void), &[egl::ATTRIB_NONE])
                .map_err(|e| format!("eglCreateImage: {e}"))?;
            buffer.image = Some(image);
            let (mut fourcc, mut planes, mut modifier) = (0i32, 0i32, 0u64);
            let (mut fds, mut strides, mut offsets) = ([-1i32; 4], [0i32; 4], [0i32; 4]);
            let ok = (f.ExportQuery)(self.display.as_ptr(), image.as_ptr(), &mut fourcc, &mut planes, &mut modifier) != 0
                && planes == 1
                && (f.Export)(self.display.as_ptr(), image.as_ptr(), fds.as_mut_ptr(), strides.as_mut_ptr(), offsets.as_mut_ptr()) != 0;
            if !ok {
                self.drop_buffer(&mut buffer);
                return Err(format!("couldn't export it (EGL_MESA_image_dma_buf_export; {planes} planes)"));
            }
            // (RGBA8 in GL is DRM's ABGR8888: R first in memory, as wgpu's Rgba8Unorm.)
            let imported = dmabuf::import(device, &Dmabuf { fd: fds[0], width, height, stride: strides[0] as u32, offset: offsets[0] as u32, modifier, format: wgpu::TextureFormat::Rgba8Unorm });
            libc::close(fds[0]);
            match imported {
                Ok(texture) => buffer.wgpu = Some(texture),
                Err(e) => {
                    self.drop_buffer(&mut buffer);
                    return Err(format!("importing it: {e} (fourcc {fourcc:#x}, modifier {modifier:#x})"));
                }
            }
            Ok(buffer)
        }
    }

    pub fn drop_buffer(&self, buffer: &mut Buffer) {
        self.make_current();
        buffer.wgpu = None;
        if let Some(image) = buffer.image.take() {
            let _ = self.egl.destroy_image(self.display, image);
        }
        unsafe {
            (self.fns.DeleteFramebuffers)(1, &buffer.fbo);
            (self.fns.DeleteTextures)(1, &buffer.texture);
        }
    }

    /// Before an embed draws into `buffer`: its framebuffer bound, the viewport all of it.
    pub fn bind(&self, buffer: &Buffer) {
        self.make_current();
        unsafe {
            (self.fns.BindFramebuffer)(FRAMEBUFFER, buffer.fbo);
            (self.fns.Viewport)(0, 0, buffer.width as i32, buffer.height as i32);
        }
    }
}

/// A buffer GL draws into and wgpu samples (the same memory).
pub struct Buffer {
    texture: u32,
    pub fbo: u32,
    image: Option<egl::Image>,
    pub wgpu: Option<wgpu::Texture>,
    pub width: u32,
    pub height: u32,
}

// A function's address as the function it is (its type: the field's).
unsafe fn cast<F: Copy>(p: *const c_void) -> F {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*const c_void>());
    unsafe { std::mem::transmute_copy(&p) }
}

// EGL's display for the GPU at `pci_bus` (0000:01:00.0), through EGL_EXT_platform_device.
fn device_display(egl: &Egl, pci_bus: &str) -> Option<egl::Display> {
    type QueryDevices = unsafe extern "C" fn(i32, *mut *mut c_void, *mut i32) -> u32;
    type QueryDeviceString = unsafe extern "C" fn(*mut c_void, i32) -> *const std::ffi::c_char;
    let extensions = egl.query_string(None, egl::EXTENSIONS).ok()?.to_string_lossy().into_owned();
    if !extensions.contains("EGL_EXT_platform_device") {
        return None;
    }
    let query_devices: QueryDevices = unsafe { std::mem::transmute(egl.get_proc_address("eglQueryDevicesEXT")?) };
    let query_string: QueryDeviceString = unsafe { std::mem::transmute(egl.get_proc_address("eglQueryDeviceStringEXT")?) };
    let mut devices = [std::ptr::null_mut(); 16];
    let mut count = 0;
    if unsafe { query_devices(16, devices.as_mut_ptr(), &mut count) } == 0 {
        return None;
    }
    let file = |d: *mut c_void| {
        let s = unsafe { query_string(d, EGL_DRM_DEVICE_FILE_EXT) };
        (!s.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(s) }.to_string_lossy().into_owned())
    };
    let bus = |path: &str| std::fs::canonicalize(format!("/sys/class/drm/{}/device", path.rsplit('/').next()?)).ok()?.file_name()?.to_str().map(str::to_owned);
    let with_files: Vec<_> = devices[..count as usize].iter().copied().filter_map(|d| Some((d, file(d)?))).collect();
    let chosen = with_files.iter().find(|(_, f)| !pci_bus.is_empty() && bus(f).is_some_and(|b| b.eq_ignore_ascii_case(pci_bus))).or(with_files.first())?.0;
    unsafe { egl.get_platform_display(EGL_PLATFORM_DEVICE_EXT, chosen, &[egl::ATTRIB_NONE]) }.ok()
}

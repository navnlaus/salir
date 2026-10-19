//! The GPU, with wgpu (Vulkan; Direct3D 12 on Windows, Metal on macOS): one
//! device for every window; each window's frame put together from layers, each
//! a texture drawn at a place: its embeds, under the page, then the page, then
//! the page's popup (a <select>'s list).

use std::sync::Arc;
use winit::window::Window;

const SHADER: &str = r#"
struct Place { rect: vec4f, flip: f32 }; // rect: left, top, right, bottom, in clip space
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> place: Place;
struct Out { @builtin(position) pos: vec4f, @location(0) uv: vec2f };
@vertex fn vs(@builtin(vertex_index) v: u32) -> Out {
  let c = vec2f(f32(v & 1u), f32(v >> 1u)); // (a strip of 4)
  var out: Out;
  out.pos = vec4f(mix(place.rect.x, place.rect.z, c.x), mix(place.rect.y, place.rect.w, c.y), 0.0, 1.0);
  out.uv = vec2f(c.x, select(c.y, 1.0 - c.y, place.flip > 0.5));
  return out;
}
@fragment fn fs(in: Out) -> @location(0) vec4f { return textureSample(tex, samp, in.uv); }
"#;

/// The format of the textures pages are copied into (CEF's: BGRA, premultiplied).
pub const PAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    blit: wgpu::RenderPipeline,
    compose: std::cell::RefCell<Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>>,
}

impl Gpu {
    pub fn new() -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            #[cfg(target_os = "linux")]
            backends: wgpu::Backends::VULKAN, // (pages and embeds come in as dmabufs, through Vulkan)
            #[cfg(target_os = "windows")]
            backends: wgpu::Backends::DX12,
            #[cfg(target_os = "macos")]
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }))
            .map_err(|e| format!("no GPU: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("no device: {e}"))?;
        device.on_uncaptured_error(Arc::new(|e| eprintln!("salir: wgpu: {e}")));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { multisampled: false, view_dimension: wgpu::TextureViewDimension::D2, sample_type: wgpu::TextureSampleType::Float { filterable: true } },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("layer"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("layer"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let blit = pipeline(&device, &pipeline_layout, &shader, PAGE_FORMAT, None);
        Ok(Gpu { instance, adapter, device, queue, sampler, layout, pipeline_layout, shader, blit, compose: Default::default() })
    }

    /// The GPU's PCI bus id (0000:01:00.0), for GL to find the same one.
    pub fn pci_bus(&self) -> String {
        self.adapter.get_info().device_pci_bus_id
    }

    /// A texture to draw a layer from, with its place.
    pub fn layer(&self, view: &wgpu::TextureView) -> Layer {
        let place = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("place"), size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: place.as_entire_binding() },
            ],
        });
        Layer { bind, place, at: None }
    }

    /// A texture of the page's (or an embed's): drawn into, and from.
    pub fn target(&self, width: u32, height: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// `source` (a layer of a texture CEF drew into) copied into `target` (PAGE_FORMAT), and waited
    /// for: CEF may draw into the source again once we return.
    pub fn copy(&self, source: &Layer, target: &wgpu::Texture) {
        source.place(&self.queue, [-1.0, 1.0, 1.0, -1.0], false);
        let view = target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, &source.bind, &[]);
            pass.draw(0..4, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// One window's frame: `background`, then each layer at its place (in order), shown.
    pub fn compose(&self, surface: &Surface, background: wgpu::Color, layers: &[&Layer]) {
        let frame = match surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                surface.surface.configure(&self.device, &surface.config);
                t
            }
            _ => {
                surface.surface.configure(&self.device, &surface.config);
                return;
            }
        };
        let pipeline = self.compose_pipeline(surface.config.format);
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(background), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            for layer in layers {
                if layer.at.is_some() {
                    pass.set_bind_group(0, &layer.bind, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        surface.window.pre_present_notify();
        self.queue.present(frame);
    }

    fn compose_pipeline(&self, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let mut cache = self.compose.borrow_mut();
        if let Some((_, p)) = cache.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        // (Layers' pixels are premultiplied: their colours already times their alpha.)
        let p = pipeline(&self.device, &self.pipeline_layout, &self.shader, format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        cache.push((format, p.clone()));
        p
    }

    /// A window's surface, its size `width`×`height`.
    pub fn surface(&self, window: Arc<Window>, width: u32, height: u32) -> Result<Surface, String> {
        let surface = self.instance.create_surface(window.clone()).map_err(|e| format!("surface: {e}"))?;
        let caps = surface.get_capabilities(&self.adapter);
        // Not sRGB: pages' colours are as they are (as in a browser), not converted.
        let format = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm].into_iter().find(|f| caps.formats.contains(f)).unwrap_or(caps.formats[0]);
        let alpha_mode = [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::Opaque].into_iter().find(|m| caps.alpha_modes.contains(m)).unwrap_or(caps.alpha_modes[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            alpha_mode,
            width: width.max(1),
            height: height.max(1),
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::AutoVsync,
        };
        surface.configure(&self.device, &config);
        Ok(Surface { window, surface, config })
    }
}

fn pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("layer"),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// A texture to draw, and where.
pub struct Layer {
    bind: wgpu::BindGroup,
    place: wgpu::Buffer,
    at: Option<[f32; 4]>,
}

impl Layer {
    // Where it goes, in clip space (left, top, right, bottom); `flip`: it's upside down (GL's).
    fn place(&self, queue: &wgpu::Queue, rect: [f32; 4], flip: bool) {
        let data = [rect[0], rect[1], rect[2], rect[3], if flip { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
        queue.write_buffer(&self.place, 0, &data.iter().flat_map(|f| f.to_ne_bytes()).collect::<Vec<u8>>());
    }

    /// At (x, y), `w`×`h` pixels, in a window `width`×`height` pixels; None: not drawn.
    pub fn put(&mut self, queue: &wgpu::Queue, at: Option<[f32; 4]>, window: (u32, u32), flip: bool) {
        let rect = at.map(|[x, y, w, h]| {
            let (ww, wh) = (window.0.max(1) as f32, window.1.max(1) as f32);
            [x / ww * 2.0 - 1.0, 1.0 - y / wh * 2.0, (x + w) / ww * 2.0 - 1.0, 1.0 - (y + h) / wh * 2.0]
        });
        if let Some(rect) = rect
            && self.at != Some(rect)
        {
            self.place(queue, rect, flip);
        }
        self.at = rect;
    }
}

pub struct Surface {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl Surface {
    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) {
        if width > 0 && height > 0 && (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&gpu.device, &self.config);
        }
    }
}

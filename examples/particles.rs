//! An app on salir with an embed of its own, drawn with wgpu: particles moved
//! by a compute shader, pulled where the page's element is pressed (its input,
//! passed on). Plugins too, as salir's own command takes them.
//!
//!   salir-particles [--plugin KIND=PATH]... page.html

use salir::{Embed, Frame, Link, Salir, Value, WindowOptions};
use std::process::ExitCode;

const SHADER: &str = r#"
struct Particle { pos: vec2f, vel: vec2f };
struct U { pull_at: vec2f, pull: f32, dt: f32, size: vec2f, aspect: f32, _pad: f32 };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read_write> ps: array<Particle>;
@compute @workgroup_size(256) fn step(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= arrayLength(&ps)) { return; }
  var p = ps[id.x];
  // (In a world as wide as the element is: x from -aspect to aspect, y from -1 to 1; drawn with x / aspect.)
  let d = u.pull_at * vec2f(u.aspect, 1.0) - p.pos;
  let r2 = dot(d, d) + 0.003;
  let a = d * (u.pull / (r2 * sqrt(r2))) * 0.05 + vec2f(-p.pos.y, p.pos.x) * 0.15;
  p.vel = (p.vel + a * u.dt) * exp(-0.9 * u.dt);
  p.pos += p.vel * u.dt;
  if (abs(p.pos.x) > u.aspect) { p.pos.x = clamp(p.pos.x, -u.aspect, u.aspect); p.vel.x *= -0.5; }
  if (abs(p.pos.y) > 1.0) { p.pos.y = clamp(p.pos.y, -1.0, 1.0); p.vel.y *= -0.5; }
  ps[id.x] = p;
}
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
struct Out { @builtin(position) pos: vec4f, @location(0) c: vec2f, @location(1) colour: vec3f };
@vertex fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Out {
  let c = vec2f(f32(v & 1u), f32(v >> 1u)) * 2.0 - 1.0;
  let p = particles[i];
  var o: Out;
  o.pos = vec4f(p.pos / vec2f(u.aspect, 1.0) + c * u.size, 0.0, 1.0);
  o.c = c;
  o.colour = mix(vec3f(0.1, 0.3, 1.0), vec3f(1.0, 0.5, 0.15), clamp(length(p.vel) * 0.8, 0.0, 1.0));
  return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4f { return vec4f(i.colour * max(0.0, 1.0 - dot(i.c, i.c)) * 0.25, 1.0); }
"#;

struct Particles {
    link: Link,
    count: u32,
    pull_at: [f32; 2],
    pull: f32,
    last: std::time::Instant,
    gpu: Option<(wgpu::Buffer, wgpu::ComputePipeline, wgpu::RenderPipeline, wgpu::BindGroup, wgpu::BindGroup)>,
}

impl Particles {
    fn setup(&mut self, f: &Frame) {
        let d = f.device;
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let step = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &module, entry_point: Some("step"), compilation_options: Default::default(), cache: None });
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let draw = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), targets: &[Some(wgpu::ColorTargetState { format: f.format, blend: Some(wgpu::BlendState { color: add, alpha: add }), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = d.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut random = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        let data: Vec<f32> = (0..self.count).flat_map(|_| {
            let (r, t) = (random().sqrt() * 0.9, random() * std::f32::consts::TAU);
            [t.cos() * r, t.sin() * r, 0.0, 0.0]
        }).collect();
        let particles = d.create_buffer(&wgpu::BufferDescriptor { label: None, size: (data.len() * 4) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        f.queue.write_buffer(&particles, 0, &data.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>());
        let group = |layout: wgpu::BindGroupLayout| d.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: particles.as_entire_binding() }] });
        let (sg, dg) = (group(step.get_bind_group_layout(0)), group(draw.get_bind_group_layout(0)));
        self.gpu = Some((uniforms, step, draw, sg, dg));
    }
}

impl Embed for Particles {
    fn render(&mut self, f: &Frame) {
        if self.gpu.is_none() {
            self.setup(f);
        }
        let (uniforms, step, draw, sg, dg) = self.gpu.as_ref().unwrap();
        let dt = self.last.elapsed().as_secs_f32().min(1.0 / 30.0);
        self.last = std::time::Instant::now();
        let u = [self.pull_at[0], self.pull_at[1], self.pull, dt, 3.0 / f.width as f32, 3.0 / f.height as f32, f.width as f32 / f.height as f32, 0.0];
        f.queue.write_buffer(uniforms, 0, &u.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>());
        let mut encoder = f.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(step);
            pass.set_bind_group(0, sg, &[]);
            pass.dispatch_workgroups(self.count.div_ceil(256), 1, 1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: f.target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.01, g: 0.01, b: 0.02, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            pass.set_pipeline(draw);
            pass.set_bind_group(0, dg, &[]);
            pass.draw(0..4, 0..self.count);
        }
        f.queue.submit([encoder.finish()]);
        self.link.request_frame(); // (always moving)
    }

    // The element's input (the page asked for it: {input: true}), in CSS pixels from its top left.
    fn message(&mut self, message: Value) {
        let Some(input) = message.get("input") else { return };
        let n = |k: &str| input.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let (w, h) = (n("width").max(1.0), n("height").max(1.0));
        if input.get("x").is_some() {
            self.pull_at = [n("x") / w * 2.0 - 1.0, 1.0 - n("y") / h * 2.0];
        }
        match input.get("type").and_then(Value::as_str) {
            Some("pointerdown") => self.pull = if n("button") == 2.0 { -1.0 } else { 1.0 },
            Some("pointerup") | Some("pointercancel") => self.pull = 0.0,
            _ => {}
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut salir = Salir::new("salir-particles").embed("particles", |options, link| {
        // (A number, or a string: <salir-embed count="262144">.)
        let count = options.get("count").and_then(|c| c.as_u64().or_else(|| c.as_str()?.parse().ok())).unwrap_or(1 << 18) as u32;
        Ok(Box::new(Particles { link, count, pull_at: [0.0, 0.0], pull: 0.0, last: std::time::Instant::now(), gpu: None }) as Box<dyn Embed>)
    });
    let mut page = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--plugin" => {
                if let Some((kind, path)) = args.get(i + 1).and_then(|p| p.split_once('=')) {
                    salir = salir.plugin(kind, path);
                }
                i += 1;
            }
            a if a.starts_with("--") => {}
            a => page = Some(a.to_string()),
        }
        i += 1;
    }
    let url = page.map(|p| std::fs::canonicalize(&p).map(|p| format!("file://{}", p.display())).unwrap_or(p)).unwrap_or_else(|| "about:blank".into());
    let mut window = WindowOptions::new("main", url);
    window.background = [0.078, 0.086, 0.114, 1.0]; // (the demo's panels': between its embeds)
    salir.window(window).on_message(|window, message| println!("{window}: {message}")).run()
}

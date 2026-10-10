//! GPU compositing of the terminal through wgpu.
//!
//! Layout, shaping and rasterisation stay on the CPU (see `layout` and
//! `font`); this replaces only the blitting. Every primitive the renderer
//! emits becomes one instanced quad: a solid rectangle, a coverage mask from
//! a glyph atlas tinted with the foreground colour, or an RGBA image. Quads
//! are drawn in emission order, so layering matches the software canvas.

use crate::color::Rgb;
use crate::font::RasterGlyph;
use crate::render::Target;
use crate::sixel::Image;
use anyhow::{anyhow, Context};
use std::collections::HashMap;
use std::sync::Arc;
use winit::window::Window;

const SHADER: &str = r#"
struct Uniforms { screen: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var tex: texture_2d<f32>;

struct Instance {
    @location(0) rect: vec4<i32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: u32,
};

struct Vertex {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> Vertex {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    let px = vec2<f32>(inst.rect.xy) + c * vec2<f32>(inst.rect.zw);
    var out: Vertex;
    out.pos = vec4<f32>(px.x / u.screen.x * 2.0 - 1.0, 1.0 - px.y / u.screen.y * 2.0, 0.0, 1.0);
    out.uv = mix(inst.uv.xy, inst.uv.zw, c);
    out.color = inst.color;
    out.kind = inst.kind;
    return out;
}

// Output is premultiplied; the pipeline blends with ONE / ONE_MINUS_SRC_ALPHA,
// which reproduces each of the software canvas's three blends.
@fragment
fn fs_main(v: Vertex) -> @location(0) vec4<f32> {
    let s = textureSample(tex, samp, v.uv);
    switch v.kind {
        case 0u: { return vec4<f32>(v.color.rgb * v.color.a, v.color.a); }
        case 1u: { let a = s.r * v.color.a; return vec4<f32>(v.color.rgb * a, a); }
        // swash colour bitmaps carry stray colour in fully transparent
        // pixels; the software canvas skips those, so do the same.
        case 2u: { return select(vec4<f32>(0.0), s, s.a > 0.0); }
        default: { return vec4<f32>(s.rgb * s.a, s.a); }
    }
}
"#;

const KIND_SOLID: u32 = 0;
/// 8-bit coverage, tinted with the instance colour.
const KIND_MASK: u32 = 1;
/// Premultiplied RGBA, as swash produces for colour emoji.
const KIND_PREMUL: u32 = 2;
/// Straight-alpha RGBA, as sixel and kitty images arrive.
const KIND_STRAIGHT: u32 = 3;

/// Glyph atlas pages are this many pixels square.
const PAGE: u32 = 2048;
/// Images not drawn for this many frames are dropped from the GPU.
const IMAGE_TTL: u64 = 120;
/// Glyphs still on the CPU side outlive the atlas, so the atlas can simply be
/// rebuilt when it fills; this bounds it at 16 MiB of coverage masks.
const MAX_PAGES: usize = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct Instance {
    /// x, y, w, h in pixels; a glyph can start a little off the left edge.
    rect: [i16; 4],
    uv: [f32; 4],
    /// r, g, b, a as bytes; the vertex format expands them to unorm floats.
    color: [u8; 4],
    kind: u32,
}

/// Sint16x4 rect, Float32x4 uv, Unorm8x4 colour, Uint32 kind.
const INSTANCE_SIZE: u64 = 32;

impl Instance {
    /// Serialise for the vertex buffer, in the attribute order above.
    #[inline]
    fn write(&self, out: &mut Vec<u8>) {
        for v in self.rect {
            out.extend_from_slice(&v.to_ne_bytes());
        }
        for f in self.uv {
            out.extend_from_slice(&f.to_ne_bytes());
        }
        out.extend_from_slice(&self.color);
        out.extend_from_slice(&self.kind.to_ne_bytes());
    }
}

/// A texture with the bind group that presents it to the shader.
struct Tex {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// An atlas page, packed in shelves: glyphs go left to right along the current
/// shelf, and a glyph too tall for it starts a new shelf below.
struct Page {
    tex: Tex,
    format: wgpu::TextureFormat,
    x: u32,
    y: u32,
    shelf_h: u32,
}

/// Which texture a quad samples.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TexKey {
    Blank,
    Page(usize),
    Image(u64),
}

/// Where a glyph lives in the atlas.
#[derive(Clone, Copy)]
struct Loc {
    key: TexKey,
    uv: [f32; 4],
}

/// A range of instances that share a texture.
struct Run {
    key: TexKey,
    start: u32,
    end: u32,
}

pub struct Gpu {
    /// The window's surface, or an offscreen texture when headless.
    output: Output,
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    blank: Tex,
    pages: Vec<Page>,
    /// Atlas location by glyph id; ids are dense, so a table beats a map.
    glyphs: Vec<Option<Loc>>,
    images: HashMap<u64, (Tex, u64)>,
    /// This frame's instances, already serialised for the vertex buffer.
    staging: Vec<u8>,
    count: u32,
    runs: Vec<Run>,
    instance_buf: wgpu::Buffer,
    clear: wgpu::Color,
    frame: u64,
    /// Ids for glyphs too big for an atlas page, counting down from the top
    /// so they never meet the image ids counting up from one.
    next_big: u64,
}

enum Output {
    Window { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration },
    Offscreen(wgpu::Texture),
}

impl Gpu {
    /// Render into a window.
    pub fn new(window: Arc<Window>) -> anyhow::Result<Gpu> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(window).context("create surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no graphics adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("alef-term"),
            ..Default::default()
        }))
        .context("request device")?;

        let caps = surface.get_capabilities(&adapter);
        // A terminal shows its colours as given: pick a non-sRGB format so the
        // GPU blends in the same gamma-encoded space the software canvas did.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow!("surface has no formats"))?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("surface not supported by adapter"))?;
        config.format = format;
        // The app paces frames itself; a blocking present would fight it.
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        surface.configure(&device, &config);
        let (width, height) = (config.width, config.height);
        Self::init(Output::Window { surface, config }, device, queue, format, width, height)
    }

    /// Render into an offscreen texture that [`Gpu::read_back`] can fetch,
    /// for tests and tools with no window.
    pub fn headless(width: u32, height: u32) -> anyhow::Result<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            ..Default::default()
        }))
        .context("no graphics adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("alef-term"),
            ..Default::default()
        }))
        .context("request device")?;
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self::init(Output::Offscreen(texture), device, queue, format, width, height)
    }

    fn init(
        output: Output,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Gpu> {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0 => Sint16x4, 1 => Float32x4, 2 => Unorm8x4, 3 => Uint32];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // Glyphs and images map 1:1 onto pixels, so nothing is ever filtered.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: INSTANCE_SIZE * 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blank = make_tex(&device, &queue, &layout, &sampler, &uniforms, wgpu::TextureFormat::Rgba8Unorm, 1, 1, &[255; 4]);
        Ok(Gpu {
            output,
            device,
            queue,
            width,
            height,
            pipeline,
            layout,
            sampler,
            uniforms,
            blank,
            pages: Vec::new(),
            glyphs: Vec::new(),
            images: HashMap::new(),
            staging: Vec::new(),
            count: 0,
            runs: Vec::new(),
            instance_buf,
            clear: wgpu::Color::BLACK,
            frame: 0,
            next_big: u64::MAX,
        })
    }

    /// Match the surface to the window. Cheap when nothing changed.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return;
        }
        let Output::Window { surface, config } = &mut self.output else { return };
        config.width = width;
        config.height = height;
        surface.configure(&self.device, config);
        self.width = width;
        self.height = height;
    }

    /// Forget every cached glyph, for when the font size changed and the
    /// CPU-side ids will never be seen again.
    pub fn clear_glyphs(&mut self) {
        self.pages.clear();
        self.glyphs.clear();
    }

    /// Start recording a frame.
    pub fn begin(&mut self) {
        self.frame += 1;
        self.staging.clear();
        self.count = 0;
        self.runs.clear();
        let frame = self.frame;
        self.images.retain(|_, (_, used)| frame - *used < IMAGE_TTL);
        if self.pages.len() > MAX_PAGES {
            self.clear_glyphs();
        }
    }

    /// Upload what was recorded and put it on screen.
    pub fn flush(&mut self, window: &Window) -> anyhow::Result<()> {
        let Output::Window { surface, config } = &self.output else {
            return Err(anyhow!("headless"));
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.device, config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(anyhow!("surface validation error")),
        };
        let view = frame.texture.create_view(&Default::default());
        self.encode(&view);
        window.pre_present_notify();
        self.queue.present(frame);
        Ok(())
    }

    /// Render what was recorded and wait for the GPU to finish, without
    /// reading anything back. Headless only; for timing.
    pub fn render_headless(&mut self) -> anyhow::Result<()> {
        let Output::Offscreen(texture) = &self.output else {
            return Err(anyhow!("not headless"));
        };
        let view = texture.create_view(&Default::default());
        self.encode(&view);
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("poll")?;
        Ok(())
    }

    /// Render what was recorded and return the pixels as packed 0x00RRGGBB,
    /// row-major, the same layout as the software canvas. Headless only.
    pub fn read_back(&mut self) -> anyhow::Result<Vec<u32>> {
        let Output::Offscreen(texture) = &self.output else {
            return Err(anyhow!("not headless"));
        };
        let view = texture.create_view(&Default::default());
        self.encode(&view);
        let Output::Offscreen(texture) = &self.output else { unreachable!() };
        let (w, h) = (self.width, self.height);
        let stride = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (stride * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: None },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("poll")?;
        rx.recv().context("map")?.context("map")?;
        let data = slice.get_mapped_range().context("mapped range")?;
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h as usize {
            let row = &data[y * stride as usize..y * stride as usize + w as usize * 4];
            for px in row.chunks_exact(4) {
                // Bgra8Unorm: bytes are b, g, r, a.
                out.push(((px[2] as u32) << 16) | ((px[1] as u32) << 8) | px[0] as u32);
            }
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }

    /// Upload the frame's instances and encode one render pass into `view`.
    fn encode(&mut self, view: &wgpu::TextureView) {
        let screen = [self.width as f32, self.height as f32, 0.0, 0.0];
        let mut screen_bytes = [0u8; 16];
        for (i, f) in screen.iter().enumerate() {
            screen_bytes[i * 4..i * 4 + 4].copy_from_slice(&f.to_ne_bytes());
        }
        self.queue.write_buffer(&self.uniforms, 0, &screen_bytes);

        let needed = (self.staging.len() as u64).max(INSTANCE_SIZE);
        if needed > self.instance_buf.size() {
            self.instance_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: needed.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.staging.is_empty() {
            self.queue.write_buffer(&self.instance_buf, 0, &self.staging);
        }

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.instance_buf.slice(..));
            for run in &self.runs {
                let tex = match run.key {
                    TexKey::Blank => &self.blank,
                    TexKey::Page(i) => &self.pages[i].tex,
                    TexKey::Image(id) => match self.images.get(&id) {
                        Some((t, _)) => t,
                        None => continue,
                    },
                };
                pass.set_bind_group(0, &tex.bind_group, &[]);
                pass.draw(0..6, run.start..run.end);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    fn push(&mut self, key: Option<TexKey>, inst: Instance) {
        let n = self.count;
        self.count += 1;
        inst.write(&mut self.staging);
        match (self.runs.last_mut(), key) {
            // Solid quads sample nothing, so they join whatever run is open.
            (Some(run), None) => run.end = n + 1,
            (Some(run), Some(k)) if run.key == k => run.end = n + 1,
            // A run of solids adopts the first texture that follows it.
            (Some(run), Some(k)) if run.key == TexKey::Blank => {
                run.key = k;
                run.end = n + 1;
            }
            _ => self.runs.push(Run { key: key.unwrap_or(TexKey::Blank), start: n, end: n + 1 }),
        }
    }

    fn upload(&self, format: wgpu::TextureFormat, w: u32, h: u32, data: &[u8]) -> Tex {
        make_tex(&self.device, &self.queue, &self.layout, &self.sampler, &self.uniforms, format, w, h, data)
    }

    /// Place a bitmap in the atlas, opening a new page when none has room.
    fn atlas_insert(&mut self, format: wgpu::TextureFormat, w: u32, h: u32, data: &[u8]) -> Loc {
        if w > PAGE || h > PAGE {
            // Larger than a page: give it a texture of its own, keyed like an
            // image so it is dropped when it falls out of use.
            let id = self.next_big;
            self.next_big -= 1;
            let tex = self.upload(format, w, h, data);
            self.images.insert(id, (tex, self.frame));
            return Loc { key: TexKey::Image(id), uv: [0.0, 0.0, 1.0, 1.0] };
        }
        let fits = |p: &Page| {
            p.format == format && ((p.x + w + 1 <= PAGE && h <= p.shelf_h) || p.y + p.shelf_h + h + 1 <= PAGE)
        };
        let idx = match self.pages.iter().rposition(fits) {
            Some(i) => i,
            None => {
                let bpp = if format == wgpu::TextureFormat::R8Unorm { 1 } else { 4 };
                let empty = vec![0u8; (PAGE * PAGE) as usize * bpp];
                let tex = self.upload(format, PAGE, PAGE, &empty);
                self.pages.push(Page { tex, format, x: 0, y: 0, shelf_h: 0 });
                self.pages.len() - 1
            }
        };
        let page = &mut self.pages[idx];
        if !(page.x + w + 1 <= PAGE && h <= page.shelf_h) {
            page.y += page.shelf_h + 1;
            page.x = 0;
            page.shelf_h = h;
        }
        let (x, y) = (page.x, page.y);
        page.x += w + 1;
        let bpp = if format == wgpu::TextureFormat::R8Unorm { 1 } else { 4 };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &page.tex._texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * bpp), rows_per_image: None },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let s = PAGE as f32;
        Loc {
            key: TexKey::Page(idx),
            uv: [x as f32 / s, y as f32 / s, (x + w) as f32 / s, (y + h) as f32 / s],
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn make_tex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    data: &[u8],
) -> Tex {
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bpp = if format == wgpu::TextureFormat::R8Unorm { 1 } else { 4 };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * bpp), rows_per_image: None },
        size,
    );
    let view = texture.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) },
        ],
    });
    Tex { _texture: texture, bind_group }
}

fn rgba(c: Rgb) -> [u8; 4] {
    [c.0, c.1, c.2, 255]
}

impl Target for Gpu {
    fn width(&self) -> usize {
        self.width as usize
    }

    fn height(&self) -> usize {
        self.height as usize
    }

    fn clear(&mut self, c: Rgb) {
        self.clear = wgpu::Color {
            r: c.0 as f64 / 255.0,
            g: c.1 as f64 / 255.0,
            b: c.2 as f64 / 255.0,
            a: 1.0,
        };
        self.staging.clear();
        self.count = 0;
        self.runs.clear();
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
        // Snap exactly as the software canvas does, so edges land on the
        // same pixels.
        let x0 = x.round().max(0.0).min(i16::MAX as f32) as i16;
        let y0 = y.round().max(0.0).min(i16::MAX as f32) as i16;
        let x1 = (x + w).round().max(0.0).min(i16::MAX as f32) as i16;
        let y1 = (y + h).round().max(0.0).min(i16::MAX as f32) as i16;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.push(None, Instance {
            rect: [x0, y0, x1 - x0, y1 - y0],
            uv: [0.0; 4],
            color: rgba(c),
            kind: KIND_SOLID,
        });
    }

    fn glyph(&mut self, g: &RasterGlyph, ox: i32, oy: i32, fg: Rgb) {
        if ox >= self.width as i32 || oy >= self.height as i32 || ox < i16::MIN as i32 || oy < i16::MIN as i32 {
            return;
        }
        let loc = match self.glyphs.get(g.id as usize).copied().flatten() {
            Some(loc) => loc,
            None => {
                let format = if g.color { wgpu::TextureFormat::Rgba8Unorm } else { wgpu::TextureFormat::R8Unorm };
                let loc = self.atlas_insert(format, g.width, g.height, &g.data);
                if self.glyphs.len() <= g.id as usize {
                    self.glyphs.resize(g.id as usize + 1, None);
                }
                self.glyphs[g.id as usize] = Some(loc);
                loc
            }
        };
        if let TexKey::Image(id) = loc.key {
            if let Some((_, used)) = self.images.get_mut(&id) {
                *used = self.frame;
            }
        }
        self.push(Some(loc.key), Instance {
            rect: [ox as i16, oy as i16, g.width.min(i16::MAX as u32) as i16, g.height.min(i16::MAX as u32) as i16],
            uv: loc.uv,
            color: rgba(fg),
            kind: if g.color { KIND_PREMUL } else { KIND_MASK },
        });
    }

    fn image(&mut self, img: &Image, ox: i32, oy: i32) {
        if img.width == 0 || img.height == 0 || ox >= self.width as i32 || oy >= self.height as i32 {
            return;
        }
        let frame = self.frame;
        if let Some((_, used)) = self.images.get_mut(&img.id) {
            *used = frame;
        } else {
            let tex = self.upload(wgpu::TextureFormat::Rgba8Unorm, img.width, img.height, &img.rgba);
            self.images.insert(img.id, (tex, frame));
        }
        self.push(Some(TexKey::Image(img.id)), Instance {
            rect: [ox.max(i16::MIN as i32) as i16, oy.max(i16::MIN as i32) as i16, img.width.min(i16::MAX as u32) as i16, img.height.min(i16::MAX as u32) as i16],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [255; 4],
            kind: KIND_STRAIGHT,
        });
    }
}

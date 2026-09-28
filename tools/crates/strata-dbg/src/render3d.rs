//! GPU rendering of the 3D scene: instanced boxes and lines drawn with wgpu
//! into an offscreen, multisampled target with its own depth buffer, then
//! shown in the interface as an egui texture.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use glam::Mat4;

/// One box: center, size and color (sRGB, alpha for translucency).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct BoxInstance {
    pub center: [f32; 3],
    pub size: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct CubeVertex {
    pos: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light: [f32; 4],
}

const SHADER: &str = r#"
struct Uniforms { view_proj: mat4x4<f32>, light: vec4<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_box(@location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>,
          @location(2) center: vec3<f32>, @location(3) size: vec3<f32>,
          @location(4) color: vec4<f32>) -> VOut {
    var o: VOut;
    let world = center + pos * size;
    o.clip = u.view_proj * vec4<f32>(world, 1.0);
    let lambert = max(dot(normal, normalize(u.light.xyz)), 0.0);
    let shade = 0.45 + 0.55 * lambert;
    o.color = vec4<f32>(color.rgb * shade, color.a);
    return o;
}

@vertex
fn vs_line(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    return v.color;
}
"#;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    resolve: wgpu::TextureView,
    depth: wgpu::TextureView,
}

pub struct Renderer3d {
    opaque: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    cube: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: Option<(wgpu::Buffer, u64)>,
    line_buf: Option<(wgpu::Buffer, u64)>,
    targets: Option<Targets>,
    pub texture: Option<egui::TextureId>,
}

fn cube_vertices() -> Vec<CubeVertex> {
    // Unit cube from (-0.5, 0, -0.5) to (0.5, 1, 0.5): it stands on the floor.
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 1.0, 0.0], [[-0.5, 1.0, -0.5], [0.5, 1.0, -0.5], [0.5, 1.0, 0.5], [-0.5, 1.0, 0.5]]),
        ([0.0, -1.0, 0.0], [[-0.5, 0.0, 0.5], [0.5, 0.0, 0.5], [0.5, 0.0, -0.5], [-0.5, 0.0, -0.5]]),
        ([1.0, 0.0, 0.0], [[0.5, 0.0, -0.5], [0.5, 0.0, 0.5], [0.5, 1.0, 0.5], [0.5, 1.0, -0.5]]),
        ([-1.0, 0.0, 0.0], [[-0.5, 0.0, 0.5], [-0.5, 0.0, -0.5], [-0.5, 1.0, -0.5], [-0.5, 1.0, 0.5]]),
        ([0.0, 0.0, 1.0], [[0.5, 0.0, 0.5], [-0.5, 0.0, 0.5], [-0.5, 1.0, 0.5], [0.5, 1.0, 0.5]]),
        ([0.0, 0.0, -1.0], [[-0.5, 0.0, -0.5], [0.5, 0.0, -0.5], [0.5, 1.0, -0.5], [-0.5, 1.0, -0.5]]),
    ];
    let mut v = Vec::with_capacity(36);
    for (n, q) in faces {
        for i in [0, 1, 2, 0, 2, 3] {
            v.push(CubeVertex { pos: q[i], normal: n });
        }
    }
    v
}

impl Renderer3d {
    pub fn new(device: &wgpu::Device) -> Renderer3d {
        use wgpu::util::DeviceExt;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let cube = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cube"),
            contents: bytemuck::cast_slice(&cube_vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let box_layouts = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<CubeVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<BoxInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![2 => Float32x3, 3 => Float32x3, 4 => Float32x4],
            }),
        ];
        let line_layouts = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        })];
        let pipeline = |label: &str,
                        vs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        topology: wgpu::PrimitiveTopology,
                        depth_write: bool,
                        blend: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: if topology == wgpu::PrimitiveTopology::TriangleList && !blend {
                        Some(wgpu::Face::Back)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = pipeline("boxes", "vs_box", &box_layouts, wgpu::PrimitiveTopology::TriangleList, true, false);
        let translucent =
            pipeline("plates", "vs_box", &box_layouts, wgpu::PrimitiveTopology::TriangleList, false, true);
        let lines = pipeline("lines", "vs_line", &line_layouts, wgpu::PrimitiveTopology::LineList, true, true);
        Renderer3d { opaque, translucent, lines, cube, uniforms, bind_group, instances: None, line_buf: None, targets: None, texture: None }
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer, size: [u32; 2]) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let tex = |label: &str, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let msaa = tex("scene3d msaa", FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = tex(
            "scene3d color",
            FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = tex("scene3d depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        match self.texture {
            Some(id) => renderer.update_egui_texture_from_wgpu_texture(device, &resolve, wgpu::FilterMode::Linear, id),
            None => self.texture = Some(renderer.register_native_texture(device, &resolve, wgpu::FilterMode::Linear)),
        }
        self.targets = Some(Targets { size, msaa, resolve, depth });
    }

    fn upload(device: &wgpu::Device, queue: &wgpu::Queue, slot: &mut Option<(wgpu::Buffer, u64)>, data: &[u8], usage: wgpu::BufferUsages) {
        let need = (data.len() as u64).max(64);
        if slot.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            *slot = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("scene3d data"),
                    size: cap,
                    usage: usage | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        if !data.is_empty() {
            queue.write_buffer(&slot.as_ref().unwrap().0, 0, data);
        }
    }

    /// Draws the scene and returns the texture to show. Boxes with alpha 1
    /// are opaque; others are drawn afterwards, blended, without depth
    /// writes.
    pub fn render(
        &mut self,
        rs: &egui_wgpu::RenderState,
        size: [u32; 2],
        view_proj: Mat4,
        boxes: &[BoxInstance],
        lines: &[LineVertex],
        background: [f32; 4],
    ) -> Option<egui::TextureId> {
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        let device = &rs.device;
        let queue = &rs.queue;
        self.ensure_targets(device, &mut rs.renderer.write(), size);
        let opaque_count = boxes.iter().take_while(|b| b.color[3] >= 1.0).count();
        Self::upload(device, queue, &mut self.instances, bytemuck::cast_slice(boxes), wgpu::BufferUsages::VERTEX);
        Self::upload(device, queue, &mut self.line_buf, bytemuck::cast_slice(lines), wgpu::BufferUsages::VERTEX);
        let u = Uniforms { view_proj: view_proj.to_cols_array_2d(), light: [0.4, 1.0, 0.25, 0.0] };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));

        let t = self.targets.as_ref().unwrap();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene3d") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    depth_slice: None,
                    resolve_target: Some(&t.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background[0] as f64,
                            g: background[1] as f64,
                            b: background[2] as f64,
                            a: background[3] as f64,
                        }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            let inst = &self.instances.as_ref().unwrap().0;
            let stride = std::mem::size_of::<BoxInstance>() as u64;
            if opaque_count > 0 {
                pass.set_pipeline(&self.opaque);
                pass.set_vertex_buffer(0, self.cube.slice(..));
                pass.set_vertex_buffer(1, inst.slice(..opaque_count as u64 * stride));
                pass.draw(0..36, 0..opaque_count as u32);
            }
            if !lines.is_empty() {
                pass.set_pipeline(&self.lines);
                pass.set_vertex_buffer(0, self.line_buf.as_ref().unwrap().0.slice(..));
                pass.draw(0..lines.len() as u32, 0..1);
            }
            let rest = boxes.len() - opaque_count;
            if rest > 0 {
                pass.set_pipeline(&self.translucent);
                pass.set_vertex_buffer(0, self.cube.slice(..));
                pass.set_vertex_buffer(1, inst.slice(opaque_count as u64 * stride..boxes.len() as u64 * stride));
                pass.draw(0..36, 0..rest as u32);
            }
        }
        queue.submit([enc.finish()]);
        self.texture
    }
}

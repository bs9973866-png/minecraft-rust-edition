//! Window and GPU rendering backend for Rust Edition.
//! Renders a small generated voxel terrain using a GPU vertex pipeline.

use std::{collections::HashSet, sync::Arc};
use winit::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    pub vsync: bool,
    pub max_fps: u32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self { vsync: false, max_fps: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 3],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

const SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    // A simple isometric camera transform for the first voxel-rendering milestone.
    let x = (input.position.x - input.position.z) * 0.105;
    let y = (input.position.x + input.position.z) * 0.060
          + input.position.y * 0.145 - 0.22;
    let depth = 0.82 - (input.position.x + input.position.z) * 0.001
                      - input.position.y * 0.012;
    output.clip_position = vec4<f32>(x, y, depth, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
"#;

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
    depth_view: wgpu::TextureView,
}

fn push_quad(out: &mut Vec<Vertex>, points: [[f32; 3]; 4], color: [f32; 3]) {
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex { position: points[index], color });
    }
}

fn build_demo_terrain() -> Vec<Vertex> {
    let mut occupied = HashSet::new();
    for x in -4_i32..=4 {
        for z in -4_i32..=4 {
            let height = if (x * 3 + z * 5).rem_euclid(7) == 0 { 2 } else { 1 };
            for y in 0..height {
                occupied.insert((x, y, z));
            }
        }
    }

    let mut vertices = Vec::new();
    let faces: [((i32, i32, i32), [[f32; 3]; 4], [f32; 3]); 6] = [
        ((0, 1, 0), [[0.,1.,0.],[0.,1.,1.],[1.,1.,1.],[1.,1.,0.]], [0.30,0.72,0.25]),
        ((0,-1, 0), [[0.,0.,1.],[0.,0.,0.],[1.,0.,0.],[1.,0.,1.]], [0.35,0.25,0.16]),
        ((0, 0, 1), [[0.,0.,1.],[1.,0.,1.],[1.,1.,1.],[0.,1.,1.]], [0.42,0.29,0.18]),
        ((0, 0,-1), [[1.,0.,0.],[0.,0.,0.],[0.,1.,0.],[1.,1.,0.]], [0.34,0.23,0.14]),
        ((1, 0, 0), [[1.,0.,1.],[1.,0.,0.],[1.,1.,0.],[1.,1.,1.]], [0.38,0.26,0.16]),
        ((-1,0, 0), [[0.,0.,0.],[0.,0.,1.],[0.,1.,1.],[0.,1.,0.]], [0.29,0.20,0.13]),
    ];

    for &(x, y, z) in &occupied {
        for (normal, corners, color) in &faces {
            if occupied.contains(&(x + normal.0, y + normal.1, z + normal.2)) {
                continue;
            }
            let translated = corners.map(|p| [p[0] + x as f32, p[1] + y as f32, p[2] + z as f32]);
            push_quad(&mut vertices, translated, *color);
        }
    }
    vertices
}

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Rust Edition depth buffer"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    }).create_view(&wgpu::TextureViewDescriptor::default())
}

impl GpuState {
    async fn new(window: Arc<winit::window::Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window)
            .map_err(|e| format!("creating GPU surface: {e}"))?;

        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.ok_or_else(|| "no compatible GPU adapter found".to_string())?;

        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Rust Edition GPU device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
        }, None).await.map_err(|e| format!("requesting GPU device: {e}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities.formats.iter().copied().find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| "surface exposes no supported formats".to_string())?;
        let present_mode = if capabilities.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            *capabilities.present_modes.first().ok_or_else(|| "surface exposes no presentation modes".to_string())?
        };
        let alpha_mode = *capabilities.alpha_modes.first()
            .ok_or_else(|| "surface exposes no alpha modes".to_string())?;

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rust Edition voxel shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rust Edition voxel pipeline layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rust Edition voxel pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let vertices = build_demo_terrain();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rust Edition terrain vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let depth_view = create_depth_view(&device, config.width, config.height);

        Ok(Self {
            surface, device, queue, config, pipeline, vertex_buffer,
            vertex_count: vertices.len() as u32, depth_view,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 { return; }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, size.width, size.height);
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("Rust Edition frame encoder") },
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Rust Edition voxel pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.035, g: 0.025, b: 0.075, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.vertex_count, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

use wgpu::util::DeviceExt;

/// Opens a native window and renders a generated voxel terrain.
pub fn run_window() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("creating event loop: {e}"))?;
    let window = Arc::new(WindowBuilder::new()
        .with_title("Minecraft 1.8.9 — Rust Edition")
        .with_inner_size(PhysicalSize::new(1280, 720))
        .build(&event_loop).map_err(|e| format!("creating window: {e}"))?);

    let mut gpu = pollster::block_on(GpuState::new(window.clone()))?;
    eprintln!("Rust Edition voxel renderer initialized.");

    event_loop.set_control_flow(ControlFlow::Wait);
    window.request_redraw();
    let result = event_loop.run(move |event, target| match event {
        Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => target.exit(),
            WindowEvent::Resized(size) => {
                gpu.resize(size);
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                gpu.resize(window.inner_size());
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => match gpu.render() {
                Ok(()) => {}
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => gpu.resize(window.inner_size()),
                Err(wgpu::SurfaceError::OutOfMemory) => target.exit(),
                Err(wgpu::SurfaceError::Timeout) => {}
            },
            _ => {}
        },
        _ => {}
    });
    result.map_err(|e| format!("event loop failed: {e}"))
}

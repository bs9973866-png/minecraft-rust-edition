//! Window and GPU rendering backend for Rust Edition.
//! Draws a generated voxel test world with a movable first-person-style camera.

use std::sync::Arc;
use game::World;
use glam::{Mat4, Vec3};
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::WindowBuilder,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    pub vsync: bool,
    pub max_fps: u32,
}

impl Default for RenderConfig {
    fn default() -> Self { Self { vsync: false, max_fps: 0 } }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex { position: [f32; 3], color: [f32; 3] }

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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform { view_projection: [[f32; 4]; 4] }

struct Camera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
    aspect: f32,
}

impl Camera {
    fn new(width: u32, height: u32) -> Self {
        Self {
            position: Vec3::new(11.0, 10.0, 15.0),
            yaw: -2.20,
            pitch: -0.38,
            aspect: width.max(1) as f32 / height.max(1) as f32,
        }
    }

    fn forward(&self) -> Vec3 {
        Vec3::new(self.yaw.cos() * self.pitch.cos(), self.pitch.sin(),
                  self.yaw.sin() * self.pitch.cos()).normalize()
    }

    fn uniform(&self) -> CameraUniform {
        let view = Mat4::look_at_rh(self.position, self.position + self.forward(), Vec3::Y);
        let projection = Mat4::perspective_rh(70_f32.to_radians(), self.aspect, 0.1, 100.0);
        CameraUniform { view_projection: (projection * view).to_cols_array_2d() }
    }

    fn key(&mut self, key: KeyCode) {
        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin()).normalize();
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        let step = 0.65;
        match key {
            KeyCode::KeyW => self.position += forward * step,
            KeyCode::KeyS => self.position -= forward * step,
            KeyCode::KeyA => self.position -= right * step,
            KeyCode::KeyD => self.position += right * step,
            KeyCode::Space => self.position.y += step,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.position.y -= step,
            KeyCode::ArrowLeft => self.yaw -= 0.12,
            KeyCode::ArrowRight => self.yaw += 0.12,
            KeyCode::ArrowUp => self.pitch = (self.pitch + 0.08).clamp(-1.45, 1.45),
            KeyCode::ArrowDown => self.pitch = (self.pitch - 0.08).clamp(-1.45, 1.45),
            _ => {}
        }
    }
}

const SHADER: &str = r#"
struct Camera {
    view_projection: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

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
    output.clip_position = camera.view_projection * vec4<f32>(input.position, 1.0);
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
    camera: Camera,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    world: World,
}

fn push_quad(out: &mut Vec<Vertex>, points: [[f32; 3]; 4], color: [f32; 3]) {
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex { position: points[index], color });
    }
}

fn build_world_vertices(world: &World) -> Vec<Vertex> {
    let faces: [([i32; 3], [[f32; 3]; 4], [f32; 3]); 6] = [
        ([0, 1, 0], [[0.,1.,0.],[0.,1.,1.],[1.,1.,1.],[1.,1.,0.]], [0.30,0.72,0.25]),
        ([0,-1, 0], [[0.,0.,1.],[0.,0.,0.],[1.,0.,0.],[1.,0.,1.]], [0.35,0.25,0.16]),
        ([0, 0, 1], [[0.,0.,1.],[1.,0.,1.],[1.,1.,1.],[0.,1.,1.]], [0.42,0.29,0.18]),
        ([0, 0,-1], [[1.,0.,0.],[0.,0.,0.],[0.,1.,0.],[1.,1.,0.]], [0.34,0.23,0.14]),
        ([1, 0, 0], [[1.,0.,1.],[1.,0.,0.],[1.,1.,0.],[1.,1.,1.]], [0.38,0.26,0.16]),
        ([-1,0, 0], [[0.,0.,0.],[0.,0.,1.],[0.,1.,1.],[0.,1.,0.]], [0.29,0.20,0.13]),
    ];
    let mut vertices = Vec::new();
    for pos in world.iter() {
        for (normal, corners, color) in &faces {
            if world.contains(pos.offset(*normal)) { continue; }
            let translated = corners.map(|p| [p[0] + pos.x as f32, p[1] + pos.y as f32, p[2] + pos.z as f32]);
            push_quad(&mut vertices, translated, *color);
        }
    }
    vertices
}
fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Rust Edition depth buffer"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
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
            compatible_surface: Some(&surface), force_fallback_adapter: false,
        }).await.ok_or_else(|| "no compatible GPU adapter found".to_string())?;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Rust Edition GPU device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
        }, None).await.map_err(|e| format!("requesting GPU device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(wgpu::TextureFormat::is_srgb)
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| "surface exposes no formats".to_string())?;
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            *caps.present_modes.first().ok_or_else(|| "surface has no present modes".to_string())?
        };
        let alpha_mode = *caps.alpha_modes.first().ok_or_else(|| "surface has no alpha modes".to_string())?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format,
            width: size.width.max(1), height: size.height.max(1),
            present_mode, alpha_mode, view_formats: vec![], desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let camera = Camera::new(config.width, config.height);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Rust Edition camera uniform"),
            contents: bytemuck::bytes_of(&camera.uniform()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<CameraUniform>() as u64),
                }, count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"), layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel pipeline layout"), bind_group_layouts: &[&camera_layout], push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel pipeline"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: "vs_main", buffers: &[Vertex::layout()] },
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList, strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw, cull_mode: None, polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false, conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual, stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(), multiview: None,
        });

        let world = World::demo();
        let vertices = build_world_vertices(&world);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain vertex buffer"), contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let depth_view = create_depth_view(&device, config.width, config.height);
        Ok(Self { surface, device, queue, config, pipeline, vertex_buffer,
            vertex_count: vertices.len() as u32, depth_view, camera, camera_buffer, camera_bind_group, world })
    }

    fn rebuild_world_mesh(&mut self) {
        let vertices = build_world_vertices(&self.world);
        self.vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("updated voxel world vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.vertex_count = vertices.len() as u32;
    }

    fn interact(&mut self, place: bool) {
        let origin = self.camera.position.to_array();
        let direction = self.camera.forward().to_array();
        if let Some(hit) = self.world.raycast(origin, direction, 6.0) {
            if place {
                let target = hit.block.offset(hit.face);
                if !self.world.contains(target) { self.world.set_solid(target); }
            } else {
                self.world.remove(hit.block);
            }
            self.rebuild_world_mesh();
        }
    }

    fn update_camera(&mut self) {
        self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&self.camera.uniform()));
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 { return; }
        self.config.width = size.width; self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, size.width, size.height);
        self.camera.aspect = size.width as f32 / size.height as f32;
        self.update_camera();
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxel pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.035, g: 0.025, b: 0.075, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None, occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.vertex_count, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

use wgpu::util::DeviceExt;

/// Opens the client window. WASD moves the camera; arrows rotate it; Space/Shift move vertically; mouse buttons edit blocks.
pub fn run_window() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("creating event loop: {e}"))?;
    let window = Arc::new(WindowBuilder::new()
        .with_title("Minecraft 1.8.9 — Rust Edition | WASD move, arrows look")
        .with_inner_size(PhysicalSize::new(1280, 720))
        .build(&event_loop).map_err(|e| format!("creating window: {e}"))?);
    let mut gpu = pollster::block_on(GpuState::new(window.clone()))?;
    eprintln!("Rust Edition voxel renderer initialized. WASD: move, arrows: look, Space/Shift: up/down, left click: break, right click: place.");

    event_loop.set_control_flow(ControlFlow::Wait);
    window.request_redraw();
    let result = event_loop.run(move |event, target| match event {
        Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => target.exit(),
            WindowEvent::Resized(size) => { gpu.resize(size); window.request_redraw(); }
            WindowEvent::ScaleFactorChanged { .. } => { gpu.resize(window.inner_size()); window.request_redraw(); }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                match button {
                    MouseButton::Left => gpu.interact(false),
                    MouseButton::Right => gpu.interact(true),
                    _ => {}
                }
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if key == KeyCode::Escape { target.exit(); }
                    else { gpu.camera.key(key); gpu.update_camera(); window.request_redraw(); }
                }
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

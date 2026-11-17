//! Window and GPU rendering backend for Rust Edition.
//! Draws a seed-generated voxel landscape with a movable first-person-style camera.

use std::{collections::{HashMap, HashSet}, sync::{Arc, mpsc::{Receiver, TryRecvError}}, time::Instant};
use game::{BlockPos, ChunkPos, Hotbar, PlayerPhysics, ServerEvent, World, CHUNK_WIDTH};
use glam::{Mat4, Vec3};
use winit::{
    dpi::PhysicalSize,
    event::{DeviceEvent, ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, WindowBuilder},
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
    physics: PlayerPhysics,
}

impl Camera {
    fn new(width: u32, height: u32, spawn: [f32; 3]) -> Self {
        Self {
            position: Vec3::from_array(spawn) + Vec3::Y * 1.62,
            yaw: -2.20,
            pitch: -0.38,
            aspect: width.max(1) as f32 / height.max(1) as f32,
            physics: PlayerPhysics::new(spawn),
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

    fn look(&mut self, delta_x: f64, delta_y: f64) {
        let sensitivity = 0.0025;
        self.yaw -= delta_x as f32 * sensitivity;
        self.pitch = (self.pitch - delta_y as f32 * sensitivity).clamp(-1.54, 1.54);
    }

    fn teleport(&mut self, feet_position: [f32; 3]) {
        self.physics.teleport(feet_position);
        self.position = Vec3::from_array(feet_position) + Vec3::Y * 1.62;
    }

    fn update_movement(&mut self, world: &World, dt: f32, keys: &HashSet<KeyCode>, jump_pressed: bool) {
        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin()).normalize();
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        let mut movement = Vec3::ZERO;
        if keys.contains(&KeyCode::KeyW) { movement += forward; }
        if keys.contains(&KeyCode::KeyS) { movement -= forward; }
        if keys.contains(&KeyCode::KeyD) { movement += right; }
        if keys.contains(&KeyCode::KeyA) { movement -= right; }
        let movement = if movement.length_squared() > 0.0 { movement.normalize() } else { Vec3::ZERO };
        self.physics.tick(world, [movement.x, movement.z], jump_pressed, dt);
        self.position = Vec3::from_array(self.physics.position()) + Vec3::Y * 1.62;
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

const OVERLAY_SHADER: &str = r#"
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
    output.clip_position = vec4<f32>(input.position, 1.0);
    output.color = input.color;
    return output;
}
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
"#;

fn push_ui_rect(out: &mut Vec<Vertex>, left: f32, top: f32, right: f32, bottom: f32, color: [f32; 3]) {
    let points = [[left, top, 0.0], [left, bottom, 0.0], [right, bottom, 0.0], [right, top, 0.0]];
    push_quad(out, points, color);
}

fn build_overlay_vertices(selected: usize) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(120);
    // Simple centered crosshair.
    push_ui_rect(&mut out, -0.012, -0.0018, 0.012, 0.0018, [0.95, 0.95, 0.95]);
    push_ui_rect(&mut out, -0.0018, -0.018, 0.0018, 0.018, [0.95, 0.95, 0.95]);
    // Nine-slot hotbar, centered near the bottom edge.
    let slot = 0.075;
    let gap = 0.006;
    let total = slot * 9.0 + gap * 8.0;
    let start = -total * 0.5;
    let top = 0.82;
    let bottom = 0.94;
    push_ui_rect(&mut out, start - 0.012, top - 0.012, -start + 0.012, bottom + 0.012, [0.055, 0.045, 0.09]);
    for index in 0..9 {
        let left = start + index as f32 * (slot + gap);
        let right = left + slot;
        push_ui_rect(&mut out, left, top, right, bottom, [0.18, 0.16, 0.24]);
        push_ui_rect(&mut out, left + 0.004, top + 0.004, right - 0.004, bottom - 0.004,
            if index == selected { [0.20, 0.78, 0.92] } else { [0.34, 0.30, 0.42] });
        push_ui_rect(&mut out, left + 0.009, top + 0.009, right - 0.009, bottom - 0.009,
            if index == selected { [0.12, 0.10, 0.17] } else { [0.10, 0.085, 0.14] });
        // Temporary block icon; real item sprites/texture atlas come later.
        push_ui_rect(&mut out, left + 0.022, top + 0.030, right - 0.022, bottom - 0.030,
            [0.38, 0.62, 0.30]);
    }
    out
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    chunk_meshes: HashMap<ChunkPos, (wgpu::Buffer, u32)>,
    depth_view: wgpu::TextureView,
    camera: Camera,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_buffer: wgpu::Buffer,
    overlay_vertex_count: u32,
    hotbar: Hotbar,
    world: World,
}

fn push_quad(out: &mut Vec<Vertex>, points: [[f32; 3]; 4], color: [f32; 3]) {
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex { position: points[index], color });
    }
}

fn build_chunk_vertices(world: &World, chunk_pos: ChunkPos) -> Vec<Vertex> {
    let faces: [([i32; 3], [[f32; 3]; 4], f32); 6] = [
        ([0, 1, 0], [[0.,1.,0.],[0.,1.,1.],[1.,1.,1.],[1.,1.,0.]], 1.00),
        ([0,-1, 0], [[0.,0.,1.],[0.,0.,0.],[1.,0.,0.],[1.,0.,1.]], 0.55),
        ([0, 0, 1], [[0.,0.,1.],[1.,0.,1.],[1.,1.,1.],[0.,1.,1.]], 0.82),
        ([0, 0,-1], [[1.,0.,0.],[0.,0.,0.],[0.,1.,0.],[1.,1.,0.]], 0.72),
        ([1, 0, 0], [[1.,0.,1.],[1.,0.,0.],[1.,1.,0.],[1.,1.,1.]], 0.88),
        ([-1,0, 0], [[0.,0.,0.],[0.,0.,1.],[0.,1.,1.],[0.,1.,0.]], 0.78),
    ];
    let mut vertices = Vec::new();
    for pos in world.chunk_blocks(chunk_pos) {
        let base_color = match world.block(pos).block_id() {
            2 => [0.30, 0.72, 0.25], // grass
            3 => [0.48, 0.31, 0.18], // dirt
            17 => [0.48, 0.29, 0.12], // log
            18 => [0.16, 0.48, 0.18], // leaves
            _ => [0.48, 0.49, 0.53], // stone / fallback
        };
        for (normal, corners, shade) in &faces {
            if world.contains(pos.offset(*normal)) { continue; }
            let color = base_color.map(|channel| channel * shade);
            let translated = corners.map(|p| [p[0] + pos.x as f32, p[1] + pos.y as f32, p[2] + pos.z as f32]);
            push_quad(&mut vertices, translated, color);
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
    async fn new(window: Arc<winit::window::Window>, server_mode: bool) -> Result<Self, String> {
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

        const WORLD_SEED: u64 = 0x4E4F5641555241;
        let world = if server_mode {
            World::new()
        } else {
            World::generate_region(WORLD_SEED, 2)
        };
        let spawn_y = if server_mode {
            64.0
        } else {
            World::terrain_surface_height(0, 0, WORLD_SEED) as f32
        };
        let camera = Camera::new(config.width, config.height, [0.5, spawn_y, 0.5]);
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

        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hotbar and crosshair overlay shader"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_SHADER.into()),
        });
        let overlay_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay pipeline layout"), bind_group_layouts: &[], push_constant_ranges: &[],
        });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("2D overlay pipeline"), layout: Some(&overlay_layout),
            vertex: wgpu::VertexState { module: &overlay_shader, entry_point: "vs_main", buffers: &[Vertex::layout()] },
            fragment: Some(wgpu::FragmentState {
                module: &overlay_shader, entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(), multiview: None,
        });

        let hotbar = Hotbar::default();
        let overlay_vertices = build_overlay_vertices(hotbar.selected_index());
        let overlay_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hotbar overlay vertex buffer"),
            contents: bytemuck::cast_slice(&overlay_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut chunk_meshes = HashMap::new();
        for chunk_pos in world.chunk_positions() {
            let vertices = build_chunk_vertices(&world, chunk_pos);
            if vertices.is_empty() { continue; }
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vertex buffer"), contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            chunk_meshes.insert(chunk_pos, (buffer, vertices.len() as u32));
        }
        let depth_view = create_depth_view(&device, config.width, config.height);
        Ok(Self { surface, device, queue, config, pipeline, chunk_meshes,
            depth_view, camera, camera_buffer, camera_bind_group,
            overlay_pipeline, overlay_buffer, overlay_vertex_count: overlay_vertices.len() as u32,
            hotbar, world })
    }

    fn refresh_overlay(&mut self) {
        let vertices = build_overlay_vertices(self.hotbar.selected_index());
        self.overlay_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("updated hotbar overlay vertex buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.overlay_vertex_count = vertices.len() as u32;
    }

    fn rebuild_chunk_mesh(&mut self, chunk_pos: ChunkPos) {
        let vertices = build_chunk_vertices(&self.world, chunk_pos);
        if vertices.is_empty() {
            self.chunk_meshes.remove(&chunk_pos);
            return;
        }
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rebuilt chunk vertex buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.chunk_meshes.insert(chunk_pos, (buffer, vertices.len() as u32));
    }

    fn rebuild_chunk_column_and_neighbors(&mut self, chunk: ChunkPos) {
        for offset in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            self.rebuild_chunk_mesh(ChunkPos {
                x: chunk.x + offset.0,
                z: chunk.z + offset.1,
            });
        }
    }

    fn rebuild_chunk_and_neighbors(&mut self, pos: BlockPos) {
        let chunk = ChunkPos::from_block(pos.x, pos.z);
        self.rebuild_chunk_mesh(chunk);
        if pos.x.rem_euclid(CHUNK_WIDTH as i32) == 0 {
            self.rebuild_chunk_mesh(ChunkPos { x: chunk.x - 1, z: chunk.z });
        } else if pos.x.rem_euclid(CHUNK_WIDTH as i32) == CHUNK_WIDTH as i32 - 1 {
            self.rebuild_chunk_mesh(ChunkPos { x: chunk.x + 1, z: chunk.z });
        }
        if pos.z.rem_euclid(CHUNK_WIDTH as i32) == 0 {
            self.rebuild_chunk_mesh(ChunkPos { x: chunk.x, z: chunk.z - 1 });
        } else if pos.z.rem_euclid(CHUNK_WIDTH as i32) == CHUNK_WIDTH as i32 - 1 {
            self.rebuild_chunk_mesh(ChunkPos { x: chunk.x, z: chunk.z + 1 });
        }
    }

    fn interact(&mut self, place: bool) {
        let origin = self.camera.position.to_array();
        let direction = self.camera.forward().to_array();
        if let Some(hit) = self.world.raycast(origin, direction, 6.0) {
            let changed = if place {
                let target = hit.block.offset(hit.face);
                if !self.world.contains(target) {
                    self.world.set_solid(target);
                    Some(target)
                } else { None }
            } else if self.world.remove(hit.block) {
                Some(hit.block)
            } else { None };
            if let Some(pos) = changed { self.rebuild_chunk_and_neighbors(pos); }
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
            for (buffer, vertex_count) in self.chunk_meshes.values() {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*vertex_count, 0..1);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HUD overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None, occlusion_query_set: None,
            });
            pass.set_pipeline(&self.overlay_pipeline);
            pass.set_vertex_buffer(0, self.overlay_buffer.slice(..));
            pass.draw(0..self.overlay_vertex_count, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

use wgpu::util::DeviceExt;

/// Opens the client window with continuous first-person movement and mouse look.
pub fn run_window() -> Result<(), String> {
    run_window_with_username("Player")
}

/// Opens the game window and shows the selected offline profile in its title.
pub fn run_window_with_username(username: &str) -> Result<(), String> {
    run_window_internal(username, None)
}

/// Opens a server-backed window. Incoming chunk/teleport events are applied on the render thread.
pub fn run_window_with_server_events(
    username: &str,
    events: Receiver<ServerEvent>,
) -> Result<(), String> {
    run_window_internal(username, Some(events))
}

fn run_window_internal(
    username: &str,
    mut server_events: Option<Receiver<ServerEvent>>,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("creating event loop: {e}"))?;
    let title = format!("Minecraft 1.8.9 — Rust Edition | {username} | WASD + mouse");
    let window = Arc::new(WindowBuilder::new()
        .with_title(&title)
        .with_inner_size(PhysicalSize::new(1280, 720))
        .build(&event_loop).map_err(|e| format!("creating window: {e}"))?);
    let mut gpu = pollster::block_on(GpuState::new(window.clone(), server_events.is_some()))?;
    let _ = window.set_cursor_grab(CursorGrabMode::Locked)
        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
    window.set_cursor_visible(false);
    eprintln!("Rust Edition: WASD move, mouse look, Space jump, left click break, right click place, Escape quit.");

    let mut pressed_keys = HashSet::new();
    let mut jump_requested = false;
    let mut last_frame = Instant::now();
    event_loop.set_control_flow(ControlFlow::Poll);
    window.request_redraw();
    let result = event_loop.run(move |event, target| match event {
        Event::DeviceEvent { event: DeviceEvent::MouseMotion { delta }, .. } => {
            gpu.camera.look(delta.0, delta.1);
            gpu.update_camera();
        }
        Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => target.exit(),
            WindowEvent::Resized(size) => gpu.resize(size),
            WindowEvent::ScaleFactorChanged { .. } => gpu.resize(window.inner_size()),
            WindowEvent::Focused(false) => {
                pressed_keys.clear();
                jump_requested = false;
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
            }
            WindowEvent::Focused(true) => {
                let _ = window.set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
                window.set_cursor_visible(false);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                    MouseScrollDelta::PixelDelta(pos) => (pos.y / 40.0).round() as i32,
                };
                if amount != 0 {
                    gpu.hotbar.cycle(-amount);
                    gpu.refresh_overlay();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                match button {
                    MouseButton::Left => gpu.interact(false),
                    MouseButton::Right => gpu.interact(true),
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if key == KeyCode::Escape && event.state == ElementState::Pressed {
                        target.exit();
                    } else if event.state == ElementState::Pressed {
                        if !event.repeat {
                            if let Some(index) = match key {
                                KeyCode::Digit1 => Some(0), KeyCode::Digit2 => Some(1),
                                KeyCode::Digit3 => Some(2), KeyCode::Digit4 => Some(3),
                                KeyCode::Digit5 => Some(4), KeyCode::Digit6 => Some(5),
                                KeyCode::Digit7 => Some(6), KeyCode::Digit8 => Some(7),
                                KeyCode::Digit9 => Some(8), _ => None,
                            } {
                                gpu.hotbar.select(index);
                                gpu.refresh_overlay();
                            }
                            if key == KeyCode::Space { jump_requested = true; }
                        }
                        pressed_keys.insert(key);
                    } else {
                        pressed_keys.remove(&key);
                    }
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
        Event::AboutToWait => {
            if let Some(events) = server_events.as_ref() {
                loop {
                    match events.try_recv() {
                        Ok(ServerEvent::ChunkColumn(column)) => {
                            let chunk = ChunkPos { x: column.x, z: column.z };
                            gpu.world.apply_chunk_column(&column);
                            gpu.rebuild_chunk_column_and_neighbors(chunk);
                        }
                        Ok(ServerEvent::BlockChanges(changes)) => {
                            let mut touched = HashSet::new();
                            for (position, state) in changes {
                                let _ = gpu.world.set_block(position, state);
                                touched.insert(ChunkPos::from_block(position.x, position.z));
                            }
                            for chunk in touched {
                                gpu.rebuild_chunk_column_and_neighbors(chunk);
                            }
                        }
                        Ok(ServerEvent::Teleport(position)) => {
                            gpu.camera.teleport(position);
                            gpu.update_camera();
                        }
                        Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                    }
                }
            }
            let now = Instant::now();
            let dt = (now - last_frame).as_secs_f32().min(0.05);
            last_frame = now;
            gpu.camera.update_movement(&gpu.world, dt, &pressed_keys, jump_requested);
            jump_requested = false;
            gpu.update_camera();
            window.request_redraw();
        }
        _ => {}
    });
    result.map_err(|e| format!("event loop failed: {e}"))
}

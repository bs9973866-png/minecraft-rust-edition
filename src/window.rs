//! Interactive first-launch preview for validating the software renderer and input loop.
//! This is an engine smoke-test scene, not a substitute for a loaded Minecraft world.

use std::error::Error;
use std::time::Instant;

use minifb::{Key, MouseMode, Window, WindowOptions};

use crate::mesh::{ModelMesh, ModelMeshVertex};
use crate::renderer::{render_mesh, Camera, FrameBuffer};
use crate::resource_pack::FaceDirection;
use crate::texture::TextureAtlas;
use std::collections::BTreeMap;

const WIDTH: usize = 960;
const HEIGHT: usize = 600;

pub fn run_window() -> Result<(), Box<dyn Error>> {
    let mut window = Window::new(
        "Minecraft Rust Edition — Engine Preview",
        WIDTH,
        HEIGHT,
        WindowOptions { resize: true, ..WindowOptions::default() },
    )?;
    window.set_target_fps(60);

    let atlas = demo_atlas();
    let scene = demo_scene();
    let mut framebuffer = FrameBuffer::new(WIDTH as u32, HEIGHT as u32)?;
    let mut camera = Camera {
        position: [0.0, 1.65, -6.0],
        yaw: 0.0,
        pitch: 0.0,
        far_plane: 128.0,
        ..Camera::default()
    };
    let mut previous_mouse: Option<(f32, f32)> = None;
    let mut last_frame = Instant::now();

    println!("[WINDOW] preview started; WASD move, Space/Shift up/down, mouse or arrows look, Esc quits");
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f32().clamp(0.0, 0.05);
        last_frame = now;
        let keys = window.get_keys();

        let mut forward = 0.0_f32;
        let mut strafe = 0.0_f32;
        if keys.contains(&Key::W) { forward += 1.0; }
        if keys.contains(&Key::S) { forward -= 1.0; }
        if keys.contains(&Key::D) { strafe += 1.0; }
        if keys.contains(&Key::A) { strafe -= 1.0; }
        if forward != 0.0 || strafe != 0.0 {
            let length = (forward * forward + strafe * strafe).sqrt().max(1.0);
            let speed = 5.0 * dt / length;
            let yaw = camera.yaw.to_radians();
            camera.position[0] += (-yaw.sin() * forward + yaw.cos() * strafe) * speed;
            camera.position[2] += (yaw.cos() * forward + yaw.sin() * strafe) * speed;
        }
        if keys.contains(&Key::Space) { camera.position[1] += 4.0 * dt; }
        if keys.contains(&Key::LeftShift) { camera.position[1] -= 4.0 * dt; }
        if keys.contains(&Key::Left) { camera.yaw -= 90.0 * dt; }
        if keys.contains(&Key::Right) { camera.yaw += 90.0 * dt; }
        if keys.contains(&Key::Up) { camera.pitch = (camera.pitch - 90.0 * dt).clamp(-89.0, 89.0); }
        if keys.contains(&Key::Down) { camera.pitch = (camera.pitch + 90.0 * dt).clamp(-89.0, 89.0); }

        if let Some((mouse_x, mouse_y)) = window.get_mouse_pos(MouseMode::Pass) {
            if let Some((old_x, old_y)) = previous_mouse {
                camera.yaw += (mouse_x - old_x) * 0.12;
                camera.pitch = (camera.pitch + (mouse_y - old_y) * 0.12).clamp(-89.0, 89.0);
            }
            previous_mouse = Some((mouse_x, mouse_y));
        } else {
            previous_mouse = None;
        }

        let (width, height) = window.get_size();
        if width > 0 && height > 0 && (width as u32 != framebuffer.width() || height as u32 != framebuffer.height()) {
            framebuffer = FrameBuffer::new(width as u32, height as u32)?;
        }
        framebuffer.clear(0x006C_9BC4);
        let _stats = render_mesh(&mut framebuffer, &scene, &atlas, &camera)?;
        window.update_with_buffer(framebuffer.pixels(), width, height)?;
    }
    println!("[WINDOW] preview closed");
    Ok(())
}

fn demo_atlas() -> TextureAtlas {
    let width = 16_u32;
    let height = 16_u32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let alternate = ((x / 4) + (y / 4)) % 2 == 0;
            let (r, g, b) = if alternate { (92, 166, 75) } else { (66, 132, 57) };
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    TextureAtlas { width, height, rgba, regions: BTreeMap::new() }
}

fn demo_scene() -> ModelMesh {
    let mut mesh = ModelMesh::default();
    // A broad ground plane and a small arrangement of block-shaped cubes.
    push_quad(&mut mesh, [
        [-20.0, 0.0, -4.0], [20.0, 0.0, -4.0],
        [20.0, 0.0, 28.0], [-20.0, 0.0, 28.0],
    ], FaceDirection::Up);
    for x in -3..=3 {
        for z in 0..=5 {
            let height = if (x + z) % 3 == 0 { 2.0 } else { 1.0 };
            push_cube(&mut mesh, [x as f32 * 2.0, 0.0, z as f32 * 2.5 + 2.0], height);
        }
    }
    mesh
}

fn push_cube(mesh: &mut ModelMesh, origin: [f32; 3], height: f32) {
    let [x, y, z] = origin;
    let x0 = x;
    let x1 = x + 1.0;
    let y0 = y;
    let y1 = y + height;
    let z0 = z;
    let z1 = z + 1.0;
    let faces = [
        (FaceDirection::Down, [[x0,y0,z0],[x1,y0,z0],[x1,y0,z1],[x0,y0,z1]]),
        (FaceDirection::Up, [[x0,y1,z0],[x0,y1,z1],[x1,y1,z1],[x1,y1,z0]]),
        (FaceDirection::North, [[x0,y0,z0],[x0,y1,z0],[x1,y1,z0],[x1,y0,z0]]),
        (FaceDirection::South, [[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1]]),
        (FaceDirection::West, [[x0,y0,z0],[x0,y0,z1],[x0,y1,z1],[x0,y1,z0]]),
        (FaceDirection::East, [[x1,y0,z0],[x1,y1,z0],[x1,y1,z1],[x1,y0,z1]]),
    ];
    for (face, corners) in faces { push_quad(mesh, corners, face); }
}

fn push_quad(mesh: &mut ModelMesh, corners: [[f32; 3]; 4], face: FaceDirection) {
    let base = mesh.vertices.len() as u32;
    let uvs = [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]];
    for index in 0..4 {
        mesh.vertices.push(ModelMeshVertex {
            position: corners[index],
            uv: uvs[index],
            texture: "preview:blocks/grass".into(),
            face,
            cullface: None,
            tint_index: None,
            block_light: 0,
            sky_light: 15,
        });
    }
    mesh.indices.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
    mesh.quad_count += 1;
}

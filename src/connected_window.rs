//! Experimental live 1.8.9 server-world viewer.
//! Uses real protocol-47 chunk packets and the existing world/meshing pipeline.
//! Block colors are diagnostic placeholders until the resource-pack atlas is wired in.

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use minifb::{Key, MouseMode, Window, WindowOptions};

use crate::math::Vec3;
use crate::mesh::{build_chunk_mesh, ModelMesh, ModelMeshVertex};
use crate::protocol::{login_offline, PlayEvent};
use crate::renderer::{render_mesh, Camera, FrameBuffer};
use crate::resource_pack::FaceDirection;
use crate::simulation::ClientSimulation;
use crate::texture::TextureAtlas;
use crate::world::{ChunkPos, World};

const WIDTH: usize = 960;
const HEIGHT: usize = 600;

type Snapshot = (World, [f32; 3], f32, f32);

pub fn run_connected_window(address: &str, username: &str) -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mpsc::channel::<Result<Snapshot, String>>();
    let address = address.to_owned();
    let username = username.to_owned();
    thread::Builder::new().name("protocol-47-reader".into()).spawn(move || {
        let result = (|| -> Result<(), String> {
            let (host, port) = crate::protocol::parse_host_port(&address).map_err(|e| e.to_string())?;
            let login = login_offline(&host, port, &username, Duration::from_secs(15)).map_err(|e| e.to_string())?;
            let mut play = login.into_play();
            let mut simulation = ClientSimulation::new(Vec3::new(0.0, 80.0, 0.0));
            tx.send(Ok((simulation.world.clone(), [0.0, 80.0, 0.0], 0.0, 0.0))).ok();
            loop {
                let event = play.read_event().map_err(|e| e.to_string())?;
                let important = matches!(event,
                    PlayEvent::Chunk(_) | PlayEvent::ChunkBulk(_) |
                    PlayEvent::BlockChange(_) | PlayEvent::MultiBlockChange(_) |
                    PlayEvent::PlayerPositionLook(_) | PlayEvent::Respawn(_));
                if let Some(ack) = simulation.apply_play_event(&event).map_err(|e| e.to_string())? {
                    play.send_player_movement(ack, simulation.player.on_ground).map_err(|e| e.to_string())?;
                }
                if important {
                    let p = simulation.player.position;
                    tx.send(Ok((simulation.world.clone(),
                        [p.x as f32, p.y as f32 + 1.62, p.z as f32],
                        simulation.player.yaw, simulation.player.pitch))).ok();
                }
            }
        })();
        if let Err(error) = result { tx.send(Err(error)).ok(); }
    })?;

    let mut window = Window::new(
        "Minecraft Rust Edition — Live 1.8.9 Server Chunks",
        WIDTH, HEIGHT, WindowOptions { resize: true, ..WindowOptions::default() },
    )?;
    window.set_target_fps(30);
    let atlas = diagnostic_atlas();
    let mut framebuffer = FrameBuffer::new(WIDTH as u32, HEIGHT as u32)?;
    let mut world = World::new();
    let mut camera = Camera { position: [0.0, 80.0, 0.0], far_plane: 192.0, ..Camera::default() };
    let mut previous_mouse: Option<(f32, f32)> = None;
    let mut connected = false;
    let mut last_error: Option<String> = None;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        for update in rx.try_iter() {
            match update {
                Ok((next_world, position, yaw, pitch)) => {
                    world = next_world;
                    if !connected { camera.position = position; camera.yaw = yaw; camera.pitch = pitch; connected = true; }
                }
                Err(error) => last_error = Some(error),
            }
        }

        let dt = 1.0 / 30.0;
        let keys = window.get_keys();
        let mut forward = 0.0_f32;
        let mut strafe = 0.0_f32;
        if keys.contains(&Key::W) { forward += 1.0; }
        if keys.contains(&Key::S) { forward -= 1.0; }
        if keys.contains(&Key::D) { strafe += 1.0; }
        if keys.contains(&Key::A) { strafe -= 1.0; }
        let length = (forward * forward + strafe * strafe).sqrt().max(1.0);
        let speed = 5.0 * dt / length;
        let yaw = camera.yaw.to_radians();
        camera.position[0] += (-yaw.sin() * forward + yaw.cos() * strafe) * speed;
        camera.position[2] += (yaw.cos() * forward + yaw.sin() * strafe) * speed;
        if keys.contains(&Key::Space) { camera.position[1] += 4.0 * dt; }
        if keys.contains(&Key::LeftShift) { camera.position[1] -= 4.0 * dt; }
        if keys.contains(&Key::Left) { camera.yaw -= 90.0 * dt; }
        if keys.contains(&Key::Right) { camera.yaw += 90.0 * dt; }
        if keys.contains(&Key::Up) { camera.pitch = (camera.pitch - 90.0 * dt).clamp(-89.0, 89.0); }
        if keys.contains(&Key::Down) { camera.pitch = (camera.pitch + 90.0 * dt).clamp(-89.0, 89.0); }
        if let Some((mx, my)) = window.get_mouse_pos(MouseMode::Pass) {
            if let Some((ox, oy)) = previous_mouse {
                camera.yaw += (mx - ox) * 0.12;
                camera.pitch = (camera.pitch + (my - oy) * 0.12).clamp(-89.0, 89.0);
            }
            previous_mouse = Some((mx, my));
        } else { previous_mouse = None; }

        let (w, h) = window.get_size();
        if w > 0 && h > 0 && (w as u32 != framebuffer.width() || h as u32 != framebuffer.height()) {
            framebuffer = FrameBuffer::new(w as u32, h as u32)?;
        }
        framebuffer.clear(0x006C_9BC4);
        let mesh = build_world_mesh(&world);
        render_mesh(&mut framebuffer, &mesh, &atlas, &camera)?;
        window.set_title(&format!(
            "Minecraft Rust Edition — live chunks: {}{}",
            world.chunk_count(),
            last_error.as_ref().map(|e| format!(" | {e}")).unwrap_or_default()
        ));
        window.update_with_buffer(framebuffer.pixels(), w, h)?;
    }
    Ok(())
}

fn build_world_mesh(world: &World) -> ModelMesh {
    let mut output = ModelMesh::default();
    for chunk_pos in world.chunk_positions() {
        append_chunk(&mut output, world, chunk_pos);
    }
    output
}

fn append_chunk(output: &mut ModelMesh, world: &World, pos: ChunkPos) {
    let chunk = build_chunk_mesh(world, pos);
    let base = output.vertices.len() as u32;
    for vertex in &chunk.vertices {
        let block_id = (vertex.block_state >> 4).min(255) as usize;
        let tile_x = block_id % 16;
        let tile_y = block_id / 16;
        let uv = [
            (tile_x as f32 * 16.0 + 8.0) / 256.0,
            (tile_y as f32 * 16.0 + 8.0) / 256.0,
        ];
        let face = match vertex.face {
            0 => FaceDirection::Down, 1 => FaceDirection::Up,
            2 => FaceDirection::North, 3 => FaceDirection::South,
            4 => FaceDirection::West, _ => FaceDirection::East,
        };
        output.vertices.push(ModelMeshVertex {
            position: [
                vertex.position[0] + (pos.x * 16) as f32,
                vertex.position[1],
                vertex.position[2] + (pos.z * 16) as f32,
            ],
            uv,
            texture: format!("legacy:block/{block_id}"),
            face,
            cullface: Some(face),
            tint_index: None,
            block_light: vertex.block_light,
            sky_light: vertex.sky_light,
        });
    }
    output.indices.extend(chunk.indices.iter().map(|i| base + *i));
    output.quad_count += chunk.quad_count;
}

fn diagnostic_atlas() -> TextureAtlas {
    let mut rgba = vec![0_u8; 256 * 256 * 4];
    for id in 0..256_usize {
        let (r, g, b) = block_color(id as u16);
        let tx = (id % 16) * 16;
        let ty = (id / 16) * 16;
        for y in 0..16 {
            for x in 0..16 {
                let noise = if ((x / 4) + (y / 4)) % 2 == 0 { 1.0 } else { 0.88 };
                let at = ((ty + y) * 256 + tx + x) * 4;
                rgba[at] = (r as f32 * noise) as u8;
                rgba[at + 1] = (g as f32 * noise) as u8;
                rgba[at + 2] = (b as f32 * noise) as u8;
                rgba[at + 3] = 255;
            }
        }
    }
    TextureAtlas { width: 256, height: 256, rgba, regions: BTreeMap::new() }
}

fn block_color(id: u16) -> (u8, u8, u8) {
    match id {
        1 => (125, 125, 125), 2 => (92, 166, 75), 3 => (134, 96, 67),
        4 => (115, 115, 115), 5 => (154, 118, 77), 7 => (45, 45, 49),
        8 | 9 => (55, 105, 190), 10 | 11 => (235, 115, 35),
        12 => (215, 205, 155), 13 => (155, 145, 125),
        14 => (245, 205, 65), 15 => (195, 190, 180), 16 => (65, 65, 65),
        17 | 162 => (125, 95, 60), 18 | 161 => (75, 135, 55),
        20 => (175, 205, 210), 21 => (65, 85, 175), 22 => (55, 65, 155),
        24 => (215, 195, 145), 35 | 159 => (190, 65, 65),
        41 => (245, 205, 55), 42 => (195, 195, 205), 45 => (175, 75, 65),
        48 => (105, 125, 105), 49 => (45, 35, 65), 56 => (90, 205, 220),
        57 => (65, 205, 225), 58 => (135, 95, 55), 79 | 174 => (155, 205, 230),
        80 => (235, 235, 240), 82 => (105, 145, 175), 87 => (145, 65, 55),
        88 => (135, 105, 75), 89 => (245, 205, 95), 98 => (105, 105, 105),
        103 => (165, 185, 75), 110 => (105, 145, 85), 121 => (205, 195, 105),
        133 => (75, 215, 115), 152 => (175, 45, 45), 155 => (225, 225, 240),
        159 => (190, 65, 65), 162 => (125, 95, 60), 168 => (75, 155, 165),
        169 => (235, 225, 145), 172 => (185, 125, 85), 173 => (35, 35, 35),
        _ => (145, 135, 125),
    }
}

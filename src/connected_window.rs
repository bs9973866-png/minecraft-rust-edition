//! Experimental live 1.8.9 server-world viewer.
//! Uses real protocol-47 chunk packets and the existing world/meshing pipeline.
//! Block colors are diagnostic placeholders until the resource-pack atlas is wired in.

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use minifb::{Key, MouseMode, Window, WindowOptions};

use crate::math::Vec3;
use crate::mesh::{build_chunk_mesh, ModelMesh, ModelMeshVertex};
use crate::protocol::{login_offline, PlayEvent, PlayerMove};
use crate::renderer::{render_mesh, Camera, FrameBuffer};
use crate::resource_pack::FaceDirection;
use crate::simulation::ClientSimulation;
use crate::texture::TextureAtlas;
use crate::world::{ChunkPos, World};

const WIDTH: usize = 960;
const HEIGHT: usize = 600;

type Snapshot = (World, [f32; 3], f32, f32, bool);

pub fn run_connected_window(address: &str, username: &str, resource_pack: Option<&str>) -> Result<(), Box<dyn Error>> {
    let loaded_assets = if let Some(path) = resource_pack {
        let stack = crate::resource_pack::ResourcePackStack::open([path])?;
        let specs = vanilla_block_specs();
        Some(crate::asset_pipeline::LoadedBlockAssets::load_available(stack, specs)?)
    } else { None };
    let (tx, rx) = mpsc::channel::<Result<Snapshot, String>>();
    let (movement_tx, movement_rx) = mpsc::channel::<PlayerMove>();
    let address = address.to_owned();
    let username = username.to_owned();
    thread::Builder::new().name("protocol-47-reader".into()).spawn(move || {
        let result = (|| -> Result<(), String> {
            let (host, port) = crate::protocol::parse_host_port(&address).map_err(|e| e.to_string())?;
            let login = login_offline(&host, port, &username, Duration::from_secs(15)).map_err(|e| e.to_string())?;
            let mut play = login.into_play();
            let mut simulation = ClientSimulation::new(Vec3::new(0.0, 80.0, 0.0));
            loop {
                let event = play.read_event().map_err(|e| e.to_string())?;
                let server_correction = matches!(&event, PlayEvent::PlayerPositionLook(_) | PlayEvent::Respawn(_));
                let important = matches!(&event,
                    PlayEvent::Chunk(_) | PlayEvent::ChunkBulk(_) |
                    PlayEvent::BlockChange(_) | PlayEvent::MultiBlockChange(_) |
                    PlayEvent::PlayerPositionLook(_) | PlayEvent::Respawn(_));
                if let Some(ack) = simulation.apply_play_event(&event).map_err(|e| e.to_string())? {
                    play.send_player_movement(ack, simulation.player.on_ground).map_err(|e| e.to_string())?;
                }
                // Drain queued UI transforms and write only the newest one. All writes
                // stay on this network thread, so protocol frames cannot interleave.
                let mut latest_movement = None;
                while let Ok(movement) = movement_rx.try_recv() {
                    latest_movement = Some(movement);
                }
                if let Some(movement) = latest_movement {
                    simulation.player.position = Vec3::new(movement.x, movement.y, movement.z);
                    simulation.player.yaw = movement.yaw;
                    simulation.player.pitch = movement.pitch;
                    play.send_player_movement(movement, simulation.player.on_ground)
                        .map_err(|e| e.to_string())?;
                }
                if important {
                    let p = simulation.player.position;
                    tx.send(Ok((simulation.world.clone(),
                        [p.x as f32, p.y as f32 + 1.62, p.z as f32],
                        simulation.player.yaw, simulation.player.pitch, server_correction))).ok();
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
    let atlas = loaded_assets.as_ref().map(|assets| assets.atlas.clone()).unwrap_or_else(diagnostic_atlas);
    let mut framebuffer = FrameBuffer::new(WIDTH as u32, HEIGHT as u32)?;
    let mut world = World::new();
    let mut camera = Camera { position: [0.0, 80.0, 0.0], far_plane: 192.0, ..Camera::default() };
    let mut previous_mouse: Option<(f32, f32)> = None;
    let mut connected = false;
    let mut scene = crate::mesh::ModelMesh::default();
    let mut last_error: Option<String> = None;
    let mut previous_frame = Instant::now();

    while window.is_open() && !window.is_key_down(Key::Escape) {
        for update in rx.try_iter() {
            match update {
                Ok((next_world, position, yaw, pitch, server_correction)) => {
                    world = next_world;
                    scene = if let Some(assets) = &loaded_assets { build_vanilla_world_mesh(&world, assets)? } else { build_world_mesh(&world) };
                    if !connected || server_correction {
                        camera.position = position;
                        camera.yaw = yaw;
                        camera.pitch = pitch;
                        connected = true;
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }

        // Base movement on elapsed wall time, not the requested window FPS.
        // Clamp long pauses (for example, while resizing) to avoid a camera jump.
        let now = Instant::now();
        let dt = now.duration_since(previous_frame).as_secs_f32().clamp(0.0, 0.1);
        previous_frame = now;
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
        // Queue the latest camera transform for the protocol thread. Position is converted\n        // from eye height back to the server's feet-based player coordinate.\n        let _ = movement_tx.send(PlayerMove {\n            x: f64::from(camera.position[0]),\n            y: f64::from(camera.position[1]) - 1.62,\n            z: f64::from(camera.position[2]),\n            yaw: camera.yaw,\n            pitch: camera.pitch,\n        });\n\n        if let Some((mx, my)) = window.get_mouse_pos(MouseMode::Pass) {
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
        render_mesh(&mut framebuffer, &scene, &atlas, &camera)?;
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


fn vanilla_block_specs() -> Vec<crate::asset_pipeline::BlockAssetSpec> {
    use crate::asset_pipeline::BlockAssetSpec;
    use crate::world::BlockState;
    use std::collections::BTreeMap;

    // Protocol 47 stores legacy states as (block_id << 4) | metadata.
    // These entries intentionally cover stable, single-model vanilla blocks.
    // Metadata-driven families are added separately with explicit blockstate properties.
    let simple: &[(u16, &str)] = &[
        (4, "cobblestone"), (7, "bedrock"), (13, "gravel"),
        (14, "gold_ore"), (15, "iron_ore"), (16, "coal_ore"),
        (20, "glass"), (21, "lapis_ore"), (22, "lapis_block"),
        (25, "noteblock"), (30, "web"), (37, "yellow_flower"),
        (39, "brown_mushroom"), (40, "red_mushroom"), (41, "gold_block"),
        (42, "iron_block"), (45, "bricks"), (46, "tnt"),
        (47, "bookshelf"), (48, "mossy_cobblestone"), (49, "obsidian"),
        (56, "diamond_ore"), (57, "diamond_block"), (58, "crafting_table"),
        (73, "redstone_ore"), (74, "lit_redstone_ore"), (79, "ice"),
        (82, "clay"), (87, "netherrack"), (88, "soul_sand"),
        (89, "glowstone"), (90, "portal"), (103, "melon_block"),
        (111, "waterlily"), (112, "nether_brick"), (116, "enchanting_table"),
        (119, "end_portal"), (121, "end_stone"), (122, "dragon_egg"),
        (123, "redstone_lamp"), (124, "lit_redstone_lamp"),
        (129, "emerald_ore"), (133, "emerald_block"), (138, "beacon"),
        (152, "redstone_block"), (153, "quartz_ore"), (165, "slime"),
        (169, "sea_lantern"), (173, "coal_block"), (174, "packed_ice"),
    ];    let mut specs = Vec::new();
    for &(id, name) in simple {
        specs.push(BlockAssetSpec::new(
            BlockState(id << 4), format!("minecraft:{name}"), BTreeMap::new(), id as u64
        ));
    }

    // Common metadata families with their 1.8 blockstate property values.
    let families: &[(u16, &str, &[&str], &str)] = &[
        (1, "stone", &["stone", "granite", "smooth_granite", "diorite", "smooth_diorite", "andesite", "smooth_andesite"], "variant"),
        (3, "dirt", &["dirt", "coarse_dirt", "podzol"], "variant"),
        (5, "planks", &["oak", "spruce", "birch", "jungle", "acacia", "dark_oak"], "variant"),
        (12, "sand", &["sand", "red_sand"], "variant"),
        (17, "log", &["oak", "spruce", "birch", "jungle"], "variant"),
        (24, "sandstone", &["sandstone", "chiseled_sandstone", "smooth_sandstone"], "type"),
        (35, "wool", &["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "silver", "cyan", "purple", "blue", "brown", "green", "red", "black"], "color"),
        (159, "stained_hardened_clay", &["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "silver", "cyan", "purple", "blue", "brown", "green", "red", "black"], "color"),
    ];
    for &(id, name, values, property) in families {
        for (meta, value) in values.iter().enumerate() {
            let mut properties = BTreeMap::new();
            properties.insert(property.to_owned(), (*value).to_owned());
            if id == 17 { properties.insert("axis".to_owned(), "y".to_owned()); }
            specs.push(BlockAssetSpec::new(
                BlockState((id << 4) | meta as u16),
                format!("minecraft:{name}"),
                properties,
                ((id as u64) << 8) | meta as u64,
            ));
        }
    }

    // Grass block has a single state family with a snowy boolean property.
    let mut grass = BTreeMap::new();
    grass.insert("snowy".to_owned(), "false".to_owned());
    specs.push(BlockAssetSpec::new(BlockState(2 << 4), "minecraft:grass", grass, 2));
    specs
}
fn build_vanilla_world_mesh(
    world: &World,
    assets: &crate::asset_pipeline::LoadedBlockAssets,
) -> Result<ModelMesh, Box<dyn Error>> {
    let mut output = ModelMesh::default();
    for chunk_pos in world.chunk_positions() {
        let chunk = assets.build_chunk_mesh(world, chunk_pos)?;
        let base = output.vertices.len() as u32;
        output.vertices.extend(chunk.vertices);
        output.indices.extend(chunk.indices.into_iter().map(|index| index + base));
        output.quad_count += chunk.quad_count;
    }
    Ok(output)
}

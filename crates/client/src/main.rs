mod status;
mod login;

use game::{decode_chunk_column, BlockPos, BlockState, ChunkPos, InventoryItem, PlayerMovement, ServerAction, ServerEvent};
use protocol::{encode_varint, PlayPacket};
use std::{io::{self, BufRead, Write}, sync::mpsc, thread};
use renderer::RenderConfig;

fn append_chat_component(value: &serde_json::Value, output: &mut String) {
    match value {
        serde_json::Value::String(text) => output.push_str(text),
        serde_json::Value::Array(parts) => {
            for part in parts { append_chat_component(part, output); }
        }
        serde_json::Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(serde_json::Value::as_str) {
                output.push_str(text);
            } else if let Some(translation_key) = object.get("translate").and_then(serde_json::Value::as_str) {
                // The client has no language table yet; keep translation components readable.
                output.push_str(translation_key);
                if let Some(arguments) = object.get("with") {
                    output.push_str(" (");
                    append_chat_component(arguments, output);
                    output.push(')');
                }
            }
            if let Some(extra) = object.get("extra") {
                append_chat_component(extra, output);
            }
        }
        _ => {}
    }
}

fn plain_chat_text(json: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(component) => {
            let mut output = String::new();
            append_chat_component(&component, &mut output);
            if output.is_empty() { json.to_owned() } else { output }
        }
        Err(_) => json.to_owned(),
    }
}

fn start_console_chat(sender: login::PlayPacketSender) -> Result<(), Box<dyn std::error::Error>> {
    thread::Builder::new()
        .name("rust-edition-console-chat".into())
        .spawn(move || {
            let stdin = io::stdin();
            let mut input = stdin.lock();
            let mut stdout = io::stdout();
            loop {
                if write!(stdout, "\\n> ").and_then(|_| stdout.flush()).is_err() { break; }
                let mut line = String::new();
                match input.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                while line.ends_with(['\\n', '\\r']) { line.pop(); }
                if line.is_empty() { continue; }
                match sender.send_chat_message(&line) {
                    Ok(()) => println!("[YOU] {line}"),
                    Err(error) => eprintln!("[CHAT] could not send message: {error}"),
                }
            }
        })?;
    Ok(())
}

fn print_usage() {
    println!("Usage:");
    println!("  rust-edition                 Show client foundation info");
    println!("  rust-edition --window        Open the local procedural-world window");
    println!("  rust-edition --status HOST[:PORT]  Query a Minecraft server status");
    println!("  rust-edition --login HOST[:PORT] --username NAME  Test offline-mode login");
    println!("  rust-edition --play HOST[:PORT] --username NAME   Console protocol-47 Play session");
    println!("  rust-edition --join HOST[:PORT] --username NAME   Open a window and display server chunks");
}

fn parse_target(value: &str) -> Result<(&str, u16), Box<dyn std::error::Error>> {
    let (host, port) = match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') => (host, port.parse::<u16>()?),
        _ => (value, 25565),
    };
    if host.is_empty() { return Err("server host cannot be empty".into()); }
    Ok((host, port))
}

fn username_arg(args: &[String]) -> Result<&str, Box<dyn std::error::Error>> {
    args.iter().position(|arg| arg == "--username")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
        .filter(|name| valid_offline_username(name))
        .ok_or_else(|| "missing or invalid --username NAME (use 3–16 Latin letters, digits, or underscores)".into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--window") {
        let username = args.iter().position(|arg| arg == "--username")
            .and_then(|index| args.get(index + 1))
            .map(String::as_str)
            .unwrap_or("Player");
        if !valid_offline_username(username) {
            return Err("invalid offline nickname: use 3–16 Latin letters, digits, or underscores".into());
        }
        println!("Offline profile: {username}");
        renderer::run_window_with_username(username).map_err(|error| format!("renderer: {error}"))?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("--join") {
        let target = args.get(2).ok_or("missing server address; example: localhost:25565")?;
        let username = username_arg(&args)?.to_owned();
        let (host, port) = parse_target(target)?;
        let host = host.to_owned();
        let thread_host = host.clone();
        let thread_username = username.clone();
        let (event_tx, event_rx) = mpsc::channel::<ServerEvent>();
        let (control_tx, control_rx) = mpsc::channel::<(
            mpsc::Sender<PlayerMovement>,
            mpsc::Sender<ServerAction>,
            login::PlayPacketSender,
        )>();

        thread::Builder::new()
            .name("rust-edition-network".into())
            .spawn(move || {
                let mut session = match login::connect_offline(&thread_host, port, &thread_username) {
                    Ok(session) => session,
                    Err(error) => {
                        eprintln!("[NETWORK] connection failed: {error}");
                        return;
                    }
                };
                eprintln!(
                    "[NETWORK] connected to {thread_host}:{port} as {}",
                    session.player.username
                );
                if let Err(error) = session.send_client_settings() {
                    eprintln!("[NETWORK] could not send client settings: {error}");
                    return;
                }

                let packet_sender = session.packet_sender();
                let movement_sender = packet_sender.clone();
                let (movement_tx, movement_rx) = mpsc::channel::<PlayerMovement>();
                let action_sender = packet_sender.clone();
                let (action_tx, action_rx) = mpsc::channel::<ServerAction>();
                if let Err(error) = thread::Builder::new()
                    .name("rust-edition-action-writer".into())
                    .spawn(move || {
                        while let Ok(action) = action_rx.recv() {
                            if let Err(error) = action_sender.send_server_action(action) {
                                eprintln!("[NETWORK] block action send failed: {error}");
                                break;
                            }
                        }
                    })
                {
                    eprintln!("[NETWORK] could not start action writer: {error}");
                    return;
                }

                if let Err(error) = thread::Builder::new()
                    .name("rust-edition-movement-writer".into())
                    .spawn(move || {
                        while let Ok(movement) = movement_rx.recv() {
                            if let Err(error) = movement_sender.send_player_movement(movement) {
                                eprintln!("[NETWORK] movement send failed: {error}");
                                break;
                            }
                        }
                    })
                {
                    eprintln!("[NETWORK] could not start movement writer: {error}");
                    return;
                }

                if control_tx.send((movement_tx, action_tx, packet_sender)).is_err() {
                    return;
                }

                let mut has_skylight = true;
                loop {
                    let packet = match session.read_play_packet() {
                        Ok(packet) => packet,
                        Err(error) => {
                            eprintln!("[NETWORK] disconnected: {error}");
                            break;
                        }
                    };

                    match &packet {
                        PlayPacket::JoinGame(info) => {
                            has_skylight = info.dimension == 0;
                            eprintln!(
                                "[WORLD] joined: dimension={}, mode={}, level={}",
                                info.dimension, info.game_mode, info.level_type
                            );
                        }
                        PlayPacket::ChunkData {
                            x, z, ground_up, primary_bitmap, data,
                        } => {
                            match decode_chunk_column(
                                *x,
                                *z,
                                *ground_up,
                                *primary_bitmap,
                                data,
                                has_skylight,
                            ) {
                                Ok(column) => {
                                    if event_tx.send(ServerEvent::ChunkColumn(column)).is_err() {
                                        break;
                                    }
                                }
                                Err(error) => eprintln!("[CHUNK] rejected ({x}, {z}): {error}"),
                            }
                        }
                        PlayPacket::ChunkDataBulk { sky_light, chunks } => {
                            for bulk in chunks {
                                match decode_chunk_column(
                                    bulk.x,
                                    bulk.z,
                                    true,
                                    bulk.primary_bitmap,
                                    &bulk.data,
                                    *sky_light,
                                ) {
                                    Ok(column) => {
                                        if event_tx.send(ServerEvent::ChunkColumn(column)).is_err() {
                                            return;
                                        }
                                    }
                                    Err(error) => eprintln!(
                                        "[CHUNK-BULK] rejected ({}, {}): {error}",
                                        bulk.x, bulk.z
                                    ),
                                }
                            }
                        }
                        PlayPacket::BlockChange { x, y, z, state } => {
                            let change = (BlockPos::new(*x, *y, *z), BlockState(*state));
                            if event_tx.send(ServerEvent::BlockChanges(vec![change])).is_err() {
                                break;
                            }
                        }
                        PlayPacket::MultiBlockChange { changes, .. } => {
                            let changes = changes.iter()
                                .map(|change| (
                                    BlockPos::new(change.x, change.y, change.z),
                                    BlockState(change.state),
                                ))
                                .collect();
                            if event_tx.send(ServerEvent::BlockChanges(changes)).is_err() {
                                break;
                            }
                        }
                        PlayPacket::SetSlot { window_id, slot, item } => {
                            let item = item.as_ref().map(|item| InventoryItem {
                                item_id: item.item_id,
                                count: item.count,
                                damage: item.damage,
                                nbt: item.nbt.clone(),
                            });
                            if event_tx.send(ServerEvent::InventorySlot {
                                window_id: *window_id,
                                slot: *slot,
                                item,
                            }).is_err() {
                                break;
                            }
                        }
                        PlayPacket::WindowItems { window_id, items } => {
                            let items = items.iter().map(|item| item.as_ref().map(|item| InventoryItem {
                                item_id: item.item_id,
                                count: item.count,
                                damage: item.damage,
                                nbt: item.nbt.clone(),
                            })).collect();
                            if event_tx.send(ServerEvent::WindowItems {
                                window_id: *window_id,
                                items,
                            }).is_err() {
                                break;
                            }
                        }
                        PlayPacket::HeldItemChange { slot } => {
                            if event_tx.send(ServerEvent::HeldItemChange { slot: *slot }).is_err() {
                                break;
                            }
                        }
                        PlayPacket::ChatMessage { json, position } => {
                            eprintln!("[CHAT:{position}] {json}");
                        }
                        PlayPacket::Unknown { id, payload } => {
                            eprintln!(
                                "[PACKET] unhandled clientbound Play packet 0x{id:02x} ({} bytes)",
                                payload.len()
                            );
                        }
                        _ => {}
                    }

                    if let Err(error) = session.respond_to_server_packet(&packet) {
                        eprintln!("[NETWORK] could not answer server packet: {error}");
                        break;
                    }
                    if matches!(packet, PlayPacket::PlayerPositionLook { .. }) {
                        let position = session.position().map(|v| v as f32);
                        let (yaw, pitch) = session.rotation();
                        if event_tx.send(ServerEvent::Teleport { position, yaw, pitch }).is_err() {
                            break;
                        }
                    }
                }
            })?;

        // Wait until login completes so the renderer receives a live movement channel.
        let (movement_tx, action_tx, packet_sender) = control_rx
            .recv()
            .map_err(|_| "server connection ended before the game window could open")?;
        println!("Opening server world window for {username} at {host}:{port}.");
        println!("Server chunks and block updates are rendered; WASD movement, hotbar selection, mining, and held-item block interactions and placement are sent to the server.");
        let render_result =
            renderer::run_window_with_server_events(&username, event_rx, movement_tx, action_tx);
        let _ = packet_sender.shutdown();
        render_result.map_err(|error| format!("renderer: {error}"))?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("--play") {
        let target = args.get(2).ok_or("missing server address; example: localhost:25565")?;
        let username = username_arg(&args)?;
        let (host, port) = parse_target(target)?;
        let mut session = login::connect_offline(host, port, username)?;
        println!("Connected to {host}:{port} as {} (UUID {})", session.player.username, session.player.uuid);
        println!("Protocol 47 Play state active. Type a message and press Enter; commands beginning with / are sent too. Press Ctrl+C to disconnect.");
        session.send_client_settings()?;
        start_console_chat(session.packet_sender())?;

        loop {
            let packet = session.read_play_packet()?;
            session.respond_to_server_packet(&packet)?;
            match packet {
                PlayPacket::JoinGame(info) => println!(
                    "[WORLD] joined: entity={}, mode={}, dimension={}, difficulty={}, level={}",
                    info.entity_id, info.game_mode, info.dimension, info.difficulty, info.level_type
                ),
                PlayPacket::ChatMessage { json, position } =>
                    println!("[CHAT:{position}] {}", plain_chat_text(&json)),
                PlayPacket::TimeUpdate { world_age, time_of_day } =>
                    println!("[TIME] age={world_age}, time={time_of_day}"),
                PlayPacket::SpawnPosition { x, y, z } =>
                    println!("[SPAWN] {x} {y} {z}"),
                PlayPacket::UpdateHealth { health, food, food_saturation } =>
                    println!("[HEALTH] health={health}, food={food}, saturation={food_saturation}"),
                PlayPacket::PlayerPositionLook { x, y, z, yaw, pitch, flags } =>
                    println!("[POSITION] server correction ({x:.2}, {y:.2}, {z:.2}), yaw={yaw:.1}, pitch={pitch:.1}, flags=0x{flags:02x}"),
                PlayPacket::ChunkData { x, z, primary_bitmap, data, .. } =>
                    println!("[CHUNK] received ({x}, {z}), sections=0x{primary_bitmap:04x}, payload={} bytes", data.len()),
                PlayPacket::ChunkDataBulk { sky_light, chunks } =>
                    println!("[CHUNK-BULK] received {} columns (sky light: {sky_light})", chunks.len()),
                PlayPacket::BlockChange { x, y, z, state } =>
                    println!("[BLOCK] ({x}, {y}, {z}) -> state {state}"),
                PlayPacket::MultiBlockChange { chunk_x, chunk_z, changes } =>
                    println!("[BLOCKS] {} updates in chunk ({chunk_x}, {chunk_z})", changes.len()),
                PlayPacket::SetSlot { window_id, slot, item } =>
                    println!("[INVENTORY] window={window_id}, slot={slot}, item={}", item.map_or("empty".into(), |stack| format!("{} x{}", stack.item_id, stack.count))),
                PlayPacket::WindowItems { window_id, items } =>
                    println!("[INVENTORY] window={window_id}, {} slots", items.len()),
                PlayPacket::HeldItemChange { slot } =>
                    println!("[INVENTORY] server selected hotbar slot {slot}"),
                PlayPacket::KeepAlive { id } =>
                    println!("[KEEP-ALIVE] replied to id={id}"),
                PlayPacket::Unknown { id, payload } =>
                    println!("[PACKET] unhandled clientbound Play packet 0x{id:02x} ({} bytes)", payload.len()),
            }
        }
    }

    if args.get(1).map(String::as_str) == Some("--login") {
        let target = args.get(2).ok_or("missing server address; example: localhost:25565")?;
        let username = username_arg(&args)?;
        let (host, port) = parse_target(target)?;
        let accepted = login::login_offline(host, port, username)?;
        println!("Offline login accepted by {host}:{port}");
        println!("Username: {}", accepted.username);
        println!("UUID: {}", accepted.uuid);
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("--status") {
        let target = args.get(2).ok_or("missing server address; example: play.example.net:25565")?;
        let (host, port) = parse_target(target)?;
        match status::ping_server(host, port) {
            Ok(json) => {
                println!("Minecraft server status — {host}:{port}");
                if let Some(version) = json.pointer("/version/name").and_then(|v| v.as_str()) {
                    println!("Version: {version}");
                }
                if let Some(online) = json.pointer("/players/online").and_then(|v| v.as_u64()) {
                    let max = json.pointer("/players/max").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("Players: {online}/{max}");
                }
                if let Some(description) = json.get("description") {
                    let text = description.as_str()
                        .map(str::to_owned)
                        .or_else(|| description.get("text").and_then(|v| v.as_str()).map(str::to_owned));
                    if let Some(text) = text { println!("MOTD: {text}"); }
                }
                println!("Raw status JSON: {json}");
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
    }

    if args.len() > 1 {
        print_usage();
        return Ok(());
    }

    let mut encoded_protocol_version = Vec::new();
    encode_varint(47, &mut encoded_protocol_version);
    let spawn_chunk = ChunkPos::from_block(0, 0);
    let render = RenderConfig::default();

    println!("Minecraft 1.8.9 — Rust Edition");
    println!("Milestone: offline login + live protocol-47 Play session");
    println!("Target protocol: 47 (1.8.9)");
    println!("Protocol version VarInt bytes: {encoded_protocol_version:02x?}");
    println!("Initial chunk coordinate: {spawn_chunk:?}");
    println!("Renderer config: {render:?}");
    println!("Use --play HOST[:PORT] --username NAME to connect to an offline-mode 1.8.9 server.");
    Ok(())
}

fn valid_offline_username(name: &str) -> bool {
    (3..=16).contains(&name.len())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::valid_offline_username;

    #[test]
    fn accepts_valid_offline_names() {
        assert!(valid_offline_username("Player_123"));
        assert!(valid_offline_username("abc"));
    }

    #[test]
    fn rejects_invalid_offline_names() {
        assert!(!valid_offline_username("ab"));
        assert!(!valid_offline_username("has space"));
        assert!(!valid_offline_username("имя"));
        assert!(!valid_offline_username("name_that_is_too_long"));
    }
}

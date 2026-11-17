mod status;
mod login;

use game::{decode_chunk_column, ChunkPos, ServerEvent};
use protocol::{encode_varint, PlayPacket};
use std::{sync::mpsc, thread};
use renderer::RenderConfig;

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
        let username = username_arg(&args)?;
        let (host, port) = parse_target(target)?;
        let host = host.to_owned();
        let username = username.to_owned();
        let (event_tx, event_rx) = mpsc::channel::<ServerEvent>();

        thread::Builder::new()
            .name("rust-edition-network".into())
            .spawn(move || {
                let mut session = match login::connect_offline(&host, port, &username) {
                    Ok(session) => session,
                    Err(error) => {
                        eprintln!("[NETWORK] connection failed: {error}");
                        return;
                    }
                };
                eprintln!(
                    "[NETWORK] connected to {host}:{port} as {}",
                    session.player.username
                );
                if let Err(error) = session.send_client_settings() {
                    eprintln!("[NETWORK] could not send client settings: {error}");
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
                                Err(error) => {
                                    eprintln!("[CHUNK] rejected ({x}, {z}): {error}");
                                }
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
                        if event_tx.send(ServerEvent::Teleport(position)).is_err() {
                            break;
                        }
                    }
                }
            })?;

        println!("Opening server world window for {username} at {host}:{port}.");
        println!("Incoming server chunks are rendered; movement is still local and not yet synchronized.");
        renderer::run_window_with_server_events(&username, event_rx)
            .map_err(|error| format!("renderer: {error}"))?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("--play") {
        let target = args.get(2).ok_or("missing server address; example: localhost:25565")?;
        let username = username_arg(&args)?;
        let (host, port) = parse_target(target)?;
        let mut session = login::connect_offline(host, port, username)?;
        println!("Connected to {host}:{port} as {} (UUID {})", session.player.username, session.player.uuid);
        println!("Protocol 47 Play state active. Press Ctrl+C to disconnect.");
        session.send_client_settings()?;

        loop {
            let packet = session.read_play_packet()?;
            session.respond_to_server_packet(&packet)?;
            match packet {
                PlayPacket::JoinGame(info) => println!(
                    "[WORLD] joined: entity={}, mode={}, dimension={}, difficulty={}, level={}",
                    info.entity_id, info.game_mode, info.dimension, info.difficulty, info.level_type
                ),
                PlayPacket::ChatMessage { json, position } =>
                    println!("[CHAT:{position}] {json}"),
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

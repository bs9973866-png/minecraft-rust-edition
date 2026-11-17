mod status;

use game::ChunkPos;
use protocol::encode_varint;
use renderer::RenderConfig;

fn print_usage() {
    println!("Usage:");
    println!("  rust-edition                 Show client foundation info");
    println!("  rust-edition --window        Open the GPU-rendered client window");
    println!("  rust-edition --status HOST[:PORT]  Query a Minecraft server status");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--window") {
        let username = args.iter().position(|arg| arg == "--username")
            .and_then(|index| args.get(index + 1))
            .map(String::as_str)
            .unwrap_or("Player");
        println!("Offline profile: {username}");
        renderer::run_window().map_err(|error| format!("renderer: {error}"))?;
        return Ok(());
    }

    if args.get(1).map(String::as_str) == Some("--status") {
        let target = args.get(2).ok_or("missing server address; example: play.example.net:25565")?;
        let (host, port) = match target.rsplit_once(':') {
            Some((host, port)) if !host.contains(']') => (host, port.parse::<u16>()?),
            _ => (target.as_str(), 25565),
        };
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
                    if let Some(text) = text {
                        println!("MOTD: {text}");
                    }
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
    println!("Milestone: protocol status ping");
    println!("Target protocol: 47 (1.8.9)");
    println!("Protocol version VarInt bytes: {encoded_protocol_version:02x?}");
    println!("Initial chunk coordinate: {spawn_chunk:?}");
    println!("Renderer config: {render:?}");
    println!("Use --status HOST[:PORT] to test a Minecraft server.");
    Ok(())
}

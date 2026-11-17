mod status;
mod login;

use game::ChunkPos;
use protocol::encode_varint;
use renderer::RenderConfig;

fn print_usage() {
    println!("Usage:");
    println!("  rust-edition                 Show client foundation info");
    println!("  rust-edition --window        Open the GPU-rendered client window");
    println!("  rust-edition --status HOST[:PORT]  Query a Minecraft server status");
    println!("  rust-edition --login HOST[:PORT] --username NAME  Test offline-mode login");
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

    if args.get(1).map(String::as_str) == Some("--login") {
        let target = args.get(2).ok_or("missing server address; example: localhost:25565")?;
        let username = args.iter().position(|arg| arg == "--username")
            .and_then(|index| args.get(index + 1))
            .map(String::as_str)
            .ok_or("missing --username NAME")?;
        let (host, port) = match target.rsplit_once(':') {
            Some((host, port)) if !host.contains(']') => (host, port.parse::<u16>()?),
            _ => (target.as_str(), 25565),
        };
        let accepted = login::login_offline(host, port, username)?;
        println!("Offline login accepted by {host}:{port}");
        println!("Username: {}", accepted.username);
        println!("UUID: {}", accepted.uuid);
        println!("Note: this only completes the login handshake; world/play packets are not implemented yet.");
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
    println!("Milestone: status ping + offline-mode login handshake");
    println!("Target protocol: 47 (1.8.9)");
    println!("Protocol version VarInt bytes: {encoded_protocol_version:02x?}");
    println!("Initial chunk coordinate: {spawn_chunk:?}");
    println!("Renderer config: {render:?}");
    println!("Use --status HOST[:PORT] to query a server, or --login HOST[:PORT] --username NAME for offline-mode login.");
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

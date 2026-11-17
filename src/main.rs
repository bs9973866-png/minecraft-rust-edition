use std::env;
use std::process::ExitCode;
use std::time::Duration;

use minecraft_rust::protocol::{
    login_offline, parse_host_port, query_status_host, PlayEvent, ProtocolError,
};
use minecraft_rust::world::World;
use minecraft_rust::{PROTOCOL_VERSION, TARGET_VERSION};

fn print_help() {
    println!("Minecraft Rust Edition — independent client foundation");
    println!();
    println!("Usage:");
    println!("  minecraft-rust --help");
    println!("  minecraft-rust --version");
    println!("  minecraft-rust --status HOST:PORT");
    println!("  minecraft-rust --connect-offline HOST:PORT USERNAME");
    println!();
    println!("Protocol target: Minecraft Java {TARGET_VERSION} (protocol {PROTOCOL_VERSION})");
    println!("--status probes the server status protocol.");
    println!("--connect-offline opens a headless offline-mode Play session and reports world events.");
    println!("Online-mode authentication and graphical rendering are not implemented yet.");
}

fn print_play_event(event: PlayEvent, world: &World) {
    match event {
        PlayEvent::JoinGame(join) => println!(
            "[JOIN] entity={} dimension={} mode={} hardcore={} level={}",
            join.entity_id, join.dimension, join.game_mode, join.hardcore_mode, join.level_type
        ),
        PlayEvent::ChatMessage(message) => println!("[CHAT] {message}"),
        PlayEvent::TimeUpdate(time) => {
            println!("[TIME] age={} time={}", time.world_age, time.time_of_day);
        }
        PlayEvent::SpawnPosition(position) => {
            println!("[SPAWN] {}, {}, {}", position.x, position.y, position.z);
        }
        PlayEvent::Respawn(respawn) => {
            println!("[RESPAWN] dimension={} mode={}", respawn.dimension, respawn.game_mode);
        }
        PlayEvent::PlayerPositionLook(position) => println!(
            "[POSITION] x={:.3} y={:.3} z={:.3} yaw={:.1} pitch={:.1}",
            position.x, position.y, position.z, position.yaw, position.pitch
        ),
        PlayEvent::Chunk(chunk) => println!(
            "[CHUNK] {}, {} loaded; world has {} columns",
            chunk.position.x, chunk.position.z, world.chunk_count()
        ),
        PlayEvent::ChunkBulk(bulk) => println!(
            "[CHUNK BULK] {} columns received; world has {} columns",
            bulk.columns.len(), world.chunk_count()
        ),
        PlayEvent::BlockChange(change) => println!(
            "[BLOCK] {}, {}, {} -> {}",
            change.position.x, change.position.y, change.position.z, change.state.0
        ),
        PlayEvent::MultiBlockChange(change) => println!(
            "[BLOCKS] {} block changes in chunk {}, {}",
            change.records.len(), change.chunk.x, change.chunk.z
        ),
        PlayEvent::HeldItemChange(slot) => println!("[HOTBAR] selected slot {slot}"),
        PlayEvent::SetSlot(data) => {
            let item = data.item.as_ref()
                .map(|stack| format!("id={} count={} damage={}", stack.item_id, stack.count, stack.damage))
                .unwrap_or_else(|| "empty".to_owned());
            println!("[INVENTORY] window={} slot={} {item}", data.window_id, data.slot);
        }
        PlayEvent::WindowItems(data) => {
            let occupied = data.items.iter().filter(|item| item.is_some()).count();
            println!("[INVENTORY] window={} slots={} occupied={}", data.window_id, data.items.len(), occupied);
        }
        PlayEvent::OpenWindow(data) => println!(
            "[WINDOW] opened id={} type={} slots={}",
            data.window_id, data.inventory_type, data.slot_count
        ),
        PlayEvent::CloseWindow(data) => println!("[WINDOW] closed id={}", data.window_id),
        PlayEvent::WindowProperty(data) => println!(
            "[WINDOW] id={} property={} value={}",
            data.window_id, data.property, data.value
        ),
        PlayEvent::ConfirmTransaction(data) => println!(
            "[WINDOW] transaction={} accepted={}",
            data.action_number, data.accepted
        ),
        PlayEvent::KeepAliveResponded(_) => {}
        PlayEvent::Unknown { packet_id, payload } => println!(
            "[PACKET] unhandled Play packet 0x{packet_id:02X} ({} payload bytes)",
            payload.len()
        ),
    }
}

fn run_offline_connection(address: &str, username: &str) -> Result<(), String> {
    let (host, port) = parse_host_port(address).map_err(|error| error.to_string())?;
    println!("[CONNECT] {host}:{port} as {username} (offline mode)");
    let login = login_offline(&host, port, username, Duration::from_secs(120))
        .map_err(|error| error.to_string())?;
    println!(
        "[LOGIN] success: {} ({})",
        login.identity.username, login.identity.uuid
    );

    let mut play = login.into_play();
    let mut world = World::new();
    loop {
        match play.read_world_event(&mut world) {
            Ok(event) => print_play_event(event, &world),
            Err(ProtocolError::UnexpectedEof) => {
                println!("[DISCONNECT] server closed the connection");
                return Ok(());
            }
            Err(ProtocolError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                ) =>
            {
                println!("[DISCONNECT] {error}");
                return Ok(());
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some("--version") => {
            println!("Minecraft Rust Edition {} (protocol {})", TARGET_VERSION, PROTOCOL_VERSION);
            Ok(())
        }
        Some("--status") => {
            let address = args.next().ok_or_else(|| "missing address; expected HOST:PORT".to_owned())?;
            if args.next().is_some() {
                return Err("too many arguments for --status".to_owned());
            }
            let (host, port) = parse_host_port(&address).map_err(|error| error.to_string())?;
            let status = query_status_host(&host, port).map_err(|error| error.to_string())?;
            println!("{status}");
            Ok(())
        }
        Some("--connect-offline") => {
            let address = args.next().ok_or_else(|| "missing server address; expected HOST:PORT".to_owned())?;
            let username = args.next().ok_or_else(|| "missing offline username".to_owned())?;
            if args.next().is_some() {
                return Err("too many arguments for --connect-offline".to_owned());
            }
            run_offline_connection(&address, &username)
        }
        Some(option) => Err(format!("unknown option: {option}; run --help for usage")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

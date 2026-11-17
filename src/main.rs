use std::env;
use std::process::ExitCode;

use minecraft_rust::protocol::{parse_host_port, query_status_host};
use minecraft_rust::{PROTOCOL_VERSION, TARGET_VERSION};

fn print_help() {
    println!("Minecraft Rust Edition — independent client foundation");
    println!();
    println!("Usage:");
    println!("  minecraft-rust --help");
    println!("  minecraft-rust --version");
    println!("  minecraft-rust --status HOST:PORT");
    println!();
    println!("Protocol target: Minecraft Java {TARGET_VERSION} (protocol {PROTOCOL_VERSION})");
    println!("Status probing uses the native Java server status handshake.");
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

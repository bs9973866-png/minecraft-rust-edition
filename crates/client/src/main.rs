use game::ChunkPos;
use protocol::encode_varint;
use renderer::RenderConfig;

fn main() {
    let mut encoded_protocol_version = Vec::new();
    // Minecraft Java 1.8.9 uses protocol version 47.
    encode_varint(47, &mut encoded_protocol_version);

    let spawn_chunk = ChunkPos::from_block(0, 0);
    let render = RenderConfig::default();

    println!("Minecraft 1.8.9 — Rust Edition");
    println!("Milestone: native client foundation");
    println!("Target protocol: 47 (1.8.9)");
    println!("Protocol version VarInt bytes: {encoded_protocol_version:02x?}");
    println!("Initial chunk coordinate: {spawn_chunk:?}");
    println!("Renderer config: {render:?}");
    println!("Next milestone: window + GPU clear pass, then protocol status/login.");
}

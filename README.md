# Minecraft Rust Edition

An independent, clean-room Rust implementation project targeting the **Minecraft Java 1.8.9 protocol (47)**.

> **Status:** early engine/protocol foundation. This is not yet a complete playable Minecraft client. Do not treat it as a drop-in replacement for Mojang's client.

## Current implementation

- Protocol 47 VarInt encoding/decoding and bounded packet framing.
- Handshake and status-request packet construction.
- TCP server status query (`--status HOST:PORT`).
- Protocol-47 chunk-column decoder for legacy block states, section masks, block light, optional skylight, and biome arrays.
- Clientbound single-block (`0x23`), multi-block (`0x22`), and bulk-chunk (`0x26`) decoders for protocol 47.
- World application of chunk batches and authoritative single/multiple block updates.
- Join Game, Keep Alive, Time Update, Spawn Position, and Player Position & Look codecs with dimension-aware skylight selection.
- Stateful TCP packet framing that switches between normal and negotiated zlib compression.
- Offline login-start, Login Success, Login Disconnect, and Set Compression codecs.
- TCP offline-mode login flow using handshake + login start, negotiated compression, and a persistent Play-state packet stream; online-mode encryption is explicitly refused.
- Play-state packet dispatcher for Join Game, chat, time, spawn position, respawn, player teleport, chunks, block updates, and bulk chunks; Keep Alive is automatically echoed.
- World-event pump that applies incoming chunk and block updates to the world model.
- World application for full and partial chunk updates, with packed light-nibble access.
- Bounded packet cursor with primitive and UTF-8 string readers.
- Sparse 16×256×16 chunk/world storage with correct negative-coordinate chunk math.
- Vector/AABB primitives and 3D voxel DDA raycasting.
- Unit tests for protocol framing, packet parsing, negative coordinates, sparse chunks, and raycasting.

## Build

Requires stable Rust and Cargo (edition 2021).

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo build --release
```

Run:

```sh
cargo run -- --help
cargo run -- --version
cargo run -- --status play.example.net:25565
cargo run -- --connect-offline 127.0.0.1:25565 PlayerName
```

The status command performs the Minecraft Java status handshake and prints the raw JSON response. The connect command is a headless diagnostic session for offline-mode servers: it logs in, answers Keep Alive, decodes play events, and applies received chunk/block changes to the in-memory world. It does not render the game or provide player controls. Online-mode authentication is not implemented yet.

## Planned engine milestones

1. Finish protocol 47 packet coverage, compression, encryption/authentication interfaces, and robust connection state handling.
2. Decode chunk sections, palettes/legacy block states, lighting, metadata, NBT, inventories, and entities.
3. Implement player physics, collision, interaction, inventory, and world ticking.
4. Add a Rust renderer, input, audio, HUD, menus, and resource-pack loading.
5. Validate behavior against a legitimately installed Minecraft 1.8.9 client and compatible test servers.

## Clean-room and asset policy

This repository contains only independently written Rust source and project documentation. It does **not** include Minecraft's game JAR, Mojang assets, a Java runtime, Forge, OptiFine, mod JARs, or extracted proprietary game code. The supplied NanoMC archive is a prebuilt portable distribution, not the Java source code for the game; it can be used as a behavioral reference where lawful, but its binaries are not copied here.

The project targets protocol compatibility. It is not affiliated with or endorsed by Mojang or Microsoft. Users must obtain game files and assets through legitimate channels and comply with their applicable licenses and terms.

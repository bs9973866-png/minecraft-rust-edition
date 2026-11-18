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
- Serverbound movement (position, look, position+look), mining status, held-hotbar slot, and animation packets; PlaySession exposes send methods.
- Protocol-47 ItemStack decoding with optional NBT, plus Set Slot (`0x2F`) and Window Items (`0x30`) inventory sync; PlaySession tracks open-window slots and carried cursor item.
- Server-driven entity synchronization: player/mob/object spawn, relative movement, rotation, teleport, head look, metadata, velocity, equipment and batch despawn; World stores entities by server entity ID and advances smooth three-tick transform interpolation.
- Open/Close Window (`0x2D`/`0x2E`), Window Property (`0x31`), and Confirm Transaction (`0x32`) codecs, including acknowledgement of rejected transactions.
- World application for full and partial chunk updates, with packed light-nibble access.
- Bounded packet cursor with primitive and UTF-8 string readers.
- Sparse 16×256×16 chunk/world storage with correct negative-coordinate chunk math.
- Vector/AABB primitives and 3D voxel DDA raycasting.
- Initial player physics: normalized directional input, ground/air acceleration, gravity, jump impulse, friction and axis-clipped collisions against full cubes plus common 1.8.9 partial shapes (slabs, snow layers, carpets, soul sand, cactus, cake and beds). Stairs, fences, doors, panes and connected block shapes remain future work.
- ClientSimulation joins player physics, world/chunk state, entity interpolation and incoming Play events; S08 relative position/rotation flags are applied and answered with the C06 movement acknowledgement.
- CPU chunk mesher creates indexed visible-face quad meshes for full-cube blocks and removes shared interior faces, including across chunk boundaries.
- Resource-pack JSON loading reads 1.8.9 blockstate variants and parented block models from extracted or ZIP packs, including layered priority/fallback. The CPU asset pipeline connects an explicit protocol-47 legacy-state mapping to weighted blockstate selection, model inheritance, shared PNG atlas construction, blockstate X/Y rotations, adjacent-cube face culling, atlas UV remapping and per-vertex block/sky light sampling for a complete chunk mesh. No Mojang assets are bundled; assets are loaded from user-provided packs. A dependency-free software triangle renderer now projects atlas-textured model meshes through a yaw/pitch camera into a depth-tested framebuffer, including near/far clipping, perspective-correct UV/light interpolation and alpha blending. The player simulation exposes camera-look direction and crosshair block raycasting from eye height.
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

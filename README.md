# Minecraft 1.8.9 — Rust Edition

A clean-room Rust reimplementation project targeting compatibility with Minecraft Java Edition 1.8.9 servers.

> This repository is an independent implementation. It does not include Mojang source code or game assets. Users must provide legally obtained game assets where required.

## Project status

Early development — repository scaffold and architecture are being established. No playable client or server compatibility is available yet.

## Goals

- Connect to unmodified Minecraft Java 1.8.9 multiplayer servers.
- Implement the 1.8.9-compatible login, status, and play protocol.
- Build a native Rust client with a measured, low-overhead rendering pipeline.
- Load user-provided resource packs and supported 1.8.9 assets.
- Keep gameplay behavior and packet handling testable and deterministic.

## Planned workspace

- `crates/protocol` — packet framing, VarInts, protocol states, codecs.
- `crates/game` — world, chunks, entities, input, and gameplay model.
- `crates/renderer` — GPU rendering and resource management.
- `crates/client` — desktop application and orchestration.

## Current milestone: editable voxel test world

The client now uses a separate sparse world model in the game crate. It supports block coordinates, adding/removing solid blocks, and voxel DDA raycasting. The renderer builds visible cube faces from that world and updates its GPU vertex buffer after edits. First-person controls now use continuous time-based movement while keys are held, and relative mouse motion rotates the camera with a captured cursor.

Controls in `--window` mode:

- `W`, `A`, `S`, `D`: move horizontally
- Arrow keys: look around
- `Space` / `Shift`: move up / down
- Left mouse button: remove the block under the crosshair ray
- Right mouse button: place a block on the face being targeted
- `Escape`: close the window

The current scene remains generated test terrain. It is not yet Minecraft's actual chunk data, textures, terrain generator, player collision/physics, or multiplayer world.

Run it with:

```sh
cargo run -p rust-edition-client -- --window
```

The window uses the platform's available graphics backend. A compatible GPU/driver and desktop session are required.

## Previous milestone: server status ping

The client now has a `--status` command that performs the Minecraft Handshake (protocol 47, next state Status), sends Status Request, reads the framed Status Response, and parses its JSON. This is a first real network-protocol milestone; it does **not** yet implement player login or gameplay.

Example:

```sh
cargo run -p rust-edition-client -- --status play.example.net:25565
```

The port is optional and defaults to `25565`:

```sh
cargo run -p rust-edition-client -- --status localhost
```

## Development

Install a recent stable Rust toolchain, then run:

```sh
cargo check --workspace
cargo test --workspace
```

## Compatibility note

The initial milestone is a protocol test harness and a native window. Full gameplay, authentication, rendering parity, and broad server compatibility will be implemented incrementally and verified against real 1.8.9 servers.

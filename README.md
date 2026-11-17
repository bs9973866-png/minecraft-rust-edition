# Minecraft 1.8.9 — Rust Edition

A clean-room Rust reimplementation project targeting compatibility with Minecraft Java Edition 1.8.9 servers.

> This repository is an independent implementation. It does not include Mojang source code or game assets. Users must provide legally obtained game assets where required.

## Project status

Early multiplayer prototype — the native client can join offline-mode protocol-47 servers, decode and render server chunk columns, apply block and inventory updates, synchronize the selected hotbar slot, send player movement, mine blocks, right-click using the server-reported held item, send/receive chat, and reset loaded chunks after a dimension respawn. It is not yet a complete Minecraft 1.8.9 implementation: online-mode authentication, in-game chat overlay, container inventory UI, entities, and many gameplay systems remain.

## Goals

- Connect to unmodified Minecraft Java 1.8.9 multiplayer servers.
- Implement the 1.8.9-compatible login, status, and play protocol.
- Build a native Rust client with a measured, low-overhead rendering pipeline.
- Load user-provided resource packs and supported 1.8.9 assets.
- Keep gameplay behavior and packet handling testable and deterministic.

## Workspace

- `crates/protocol` — packet framing, VarInts, protocol states, codecs.
- `crates/game` — world, chunks, entities, input, and gameplay model.
- `crates/renderer` — GPU rendering and resource management.
- `crates/client` — game client executable.
- `crates/launcher` — small desktop launcher with a saved offline nickname.

## Current milestone: server-backed world window

Start the game against an offline-mode Minecraft Java 1.8.9 server:

```powershell
cargo run -p rust-edition-client -- --join play.example.net:25565 --username Player
```

The `--join` mode performs protocol-47 login, keeps the connection alive, receives full and partial chunk columns (including bulk chunk packets), decodes legacy 1.8 block IDs/metadata, and renders the received server world in the native 3D window. Server block-change packets and server teleports are applied to the rendered world/player. WASD movement and camera rotation are sent to the server at 20 updates per second after the initial server teleport. Number keys and the mouse wheel send the selected hotbar slot to the server. Hold the left mouse button to send start/finish/cancel digging actions. Right-click sends a protocol-47 block-placement/use packet containing the selected item stack most recently reported by the server, allowing ordinary block placement and item use where the server permits it.

The protocol decoder recognizes protocol-47 player, mob, and object spawns, entity removal, relative movement, and teleport packets. Spawn and movement updates are forwarded into a live entity-state map on the game thread. Entity models, metadata/equipment updates, interpolation, and visible entity rendering are still not implemented. The client reads player-window Set Slot / Window Items packets, preserves item NBT, tracks the nine hotbar stacks, follows server-held-slot changes using the protocol-47 S09 packet, and echoes the selected stack in right-click placement packets. It can send chat and slash commands through the attached terminal in both `--join` and `--play` modes, and it flattens basic incoming chat JSON components for readable terminal output. Protocol-47 Respawn packets reset the local chunk set when the player changes dimension. Full in-game chat UI, container inventory UI, cursor-stack handling, creative inventory operations, and full item-use behavior are not implemented yet. Microsoft/Mojang online-mode encryption and authenticated login are not implemented, so servers requiring online-mode authentication will reject this client. Entity rendering, all gameplay packets, exact vanilla movement physics/mining timings, and broad server compatibility remain incomplete.

The standalone local `--window` mode remains available for the procedural test world. Its controls are WASD, mouse look, Space to jump, 1–9 or mouse wheel to select a hotbar slot, left click to break, right click to place locally, and Escape to quit.

The game crate stores sparse 16×256×16 chunks and decodes protocol-47 section data into legacy block states. The renderer maintains per-chunk GPU meshes and rebuilds affected chunk meshes when server block updates arrive. The terrain generator used by local `--window` mode is an original test generator, not Minecraft's vanilla world generator. Texture atlases, official resource-pack support, caves, ores, fluids, full block models, and many vanilla mechanics remain future work.

## Simple launcher

The launcher is intentionally minimal: enter an offline nickname and press **Play**. It saves the nickname locally and starts the game executable with `--window --username <nickname>`. No Microsoft or Ely.by login is involved.

To build a portable Windows folder automatically, run `build-launcher.ps1` from the repository root:

```powershell
.\\build-launcher.ps1
```

The script builds both release executables and copies them into `dist/RustEdition`. Start `rust-edition-launcher.exe` there; keep `rust-edition.exe` beside it. After a successful launch, the launcher closes automatically. You can also press Enter in the nickname field to launch. GitHub Actions now builds a Windows portable artifact on pushes to `main`. You can also build manually with `cargo build --release -p rust-edition-client -p rust-edition-launcher`.

The launcher stores its nickname in `%APPDATA%/RustEdition/launcher.json`. Offline names must be 3–16 characters and contain only Latin letters, numbers, and underscores.

You can still run the game directly during development:

```sh
cargo run -p rust-edition-client -- --window
```

The offline nickname is passed to the client and shown in the game window title. The same nickname can be used for the offline-mode protocol login command documented below.

The window uses the platform's available graphics backend. A compatible GPU/driver and desktop session are required.

## Previous milestone: server status ping

The client has a `--status` command that performs the Minecraft Handshake (protocol 47, next state Status), sends Status Request, reads the framed Status Response, and parses its JSON.

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

This project is an early prototype. Inventory and block interaction support are partial, while online-mode authentication, in-game chat UI, container inventory management, entity rendering, complete vanilla mechanics, and broad server compatibility remain unimplemented and must be verified against real 1.8.9 servers.


### Offline-mode server login prototype

Connect to an offline-mode 1.8.9 server and stay in Play state:

```powershell
cargo run -p rust-edition-client -- --play localhost:25565 --username Player
```

The connection logs selected server events and chunk packet sizes to the terminal. Press `Ctrl+C` to disconnect. For a login-only handshake check, use:

```powershell
cargo run -p rust-edition-client -- --login localhost:25565 --username Player
```

Both commands work only with servers configured for offline-mode. Online-mode encryption and authenticated Microsoft login are not implemented. Login/play packet compression (Set Compression + zlib framing) is supported. In `--play`, type a chat message or `/command` into the terminal and press Enter. In `--join`, the same terminal accepts chat while the game window is open.

# Minecraft 1.8.9 — Rust Edition

A clean-room Rust reimplementation project targeting compatibility with Minecraft Java Edition 1.8.9 servers.

> This repository is an independent implementation. It does not include Mojang source code or game assets. Users must provide legally obtained game assets where required.

## Project status

Early multiplayer-capable prototype — the native window, procedural local terrain, first-person movement, collision, hotbar HUD, status ping, offline-mode Login, and a persistent protocol-47 Play connection are implemented. The Play connection receives and reports selected server packets and answers Keep Alive and position corrections, but it is not yet connected to the renderer or a complete playable multiplayer world.

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

## Current milestone: live protocol-47 Play session

`rust-edition-client --play HOST[:PORT] --username NAME` now performs the protocol-47 handshake, completes offline-mode login, and keeps the TCP connection open in Play state. It enables zlib packet compression when requested by the server, sends Client Settings, decodes Join Game, Chat Message, Time Update, Spawn Position, Update Health, Player Position And Look, Chunk Data and Keep Alive, preserves unknown packet payloads, replies to Keep Alive, and acknowledges absolute/relative server position corrections.

The Play session supports offline-mode servers only. Servers requiring Microsoft/Mojang online-mode encryption are rejected clearly; authentication is not bypassed. Chunk Data is currently retained as raw bytes and reported to the console; it is not yet decoded into blocks or displayed in the renderer. This is a real protocol connection milestone, not yet full multiplayer gameplay.

The game crate now stores world blocks in sparse 16×256×16 chunks (the classic 1.8.9 vertical build range). Chunks are created only when a non-air block is written and reclaimed when emptied. World coordinates map correctly across positive and negative chunk boundaries, and block states are stored per cell. A deterministic, seed-based heightmap generator creates rolling stone/dirt/grass terrain and simple trees, with generation performed in two passes so tree canopies can cross chunk borders. The renderer uses a five-by-five chunk test region, starts the player above the generated surface, colors the basic block types differently, and keeps a separate GPU vertex buffer for each chunk. First-person controls now use continuous time-based movement while keys are held, and relative mouse motion rotates the camera with a captured cursor.

Controls in `--window` mode:

- `W`, `A`, `S`, `D`: move horizontally
- Mouse: look around (cursor is captured)
- `Space`: jump
- `1`–`9`: select a hotbar slot
- Mouse wheel: cycle through hotbar slots
- Left mouse button: remove the block under the crosshair ray
- Right mouse button: place a block on the face being targeted
- `Escape`: close the window

The game crate now includes a nine-slot hotbar data model with selection, wraparound scrolling, and slot state IDs. The renderer draws a crosshair and a nine-slot HUD overlay (temporary placeholder icons, not Minecraft item textures). The game crate also includes a basic player AABB, gravity, jumping, and axis-separated block collision. The renderer starts the player above the generated terrain and uses Space to jump. This is still an early physics prototype: it does not yet include Minecraft's exact movement constants, step-up behavior, swimming, ladders, sprinting, or multiplayer prediction. The renderer now creates a separate GPU vertex buffer for each loaded chunk. Editing a block rebuilds only its owning chunk mesh and the directly adjacent chunk mesh when the edit touches a chunk boundary. Mesh generation enumerates only blocks stored in the target chunk, while face visibility still checks neighboring world blocks. The terrain generator is a small original prototype, not Minecraft's vanilla 1.8.9 generator. It does not yet load real server chunk packets, biomes, texture atlases, caves, ores, water, or the complete vanilla block palette.

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

This project is an early prototype. Full gameplay, online-mode authentication, real server chunk-to-block decoding, multiplayer world rendering, client movement/input synchronization, inventory interaction, entities, and broad server compatibility remain unimplemented and must be verified against real 1.8.9 servers.


### Offline-mode server login prototype

Connect to an offline-mode 1.8.9 server and stay in Play state:

```powershell
cargo run -p rust-edition-client -- --play localhost:25565 --username Player
```

The connection logs selected server events and chunk packet sizes to the terminal. Press `Ctrl+C` to disconnect. For a login-only handshake check, use:

```powershell
cargo run -p rust-edition-client -- --login localhost:25565 --username Player
```

Both commands work only with servers configured for offline-mode. Online-mode encryption and authenticated Microsoft login are not implemented. Login/play packet compression (Set Compression + zlib framing) is supported.

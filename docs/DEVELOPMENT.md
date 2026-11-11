# Development

## Toolchain

Anada builds with Rust 1.98.1 for `x86_64-pc-windows-gnu`, pinned in
`rust-toolchain.toml`, and the MinGW-w64 linker (for example from MSYS2 at
`C:/msys64/mingw64/bin/gcc.exe`). The build also needs `windres` from the same MinGW-w64
package on `PATH`; it embeds the icon. Dependencies are pinned in `Cargo.lock`.

The logo sources are `assets/anada.svg` and `assets/anada-small.svg` (thicker lines,
for sizes up to 24 pixels). After a change, run `uv run assets/build-icon.py` to
regenerate `anada.ico` and `anada.png`, and commit them.

The dev profile optimizes Rubato and `anada-dsp` at level 3 and the application at
level 2 so real-time audio keeps up in debug builds. Keep those settings.

## Build and run

    cargo build --workspace --locked
    ./target/debug/anada.exe

The window starts the sibling `anada-host.exe` when needed. Closing the window keeps
the host running; use **Exit host** or `anada.exe --stop-host` to stop it. Windows locks
running executables, so stop both before a build or test run that replaces them.

Useful modes:

    ./target/debug/anada.exe --probe        # read-only device and session inventory
    ./target/debug/anada.exe --snapshot     # current host state as JSON
    ./target/debug/anada.exe --stop-host
    ./target/debug/anada.exe --ui-demo      # sample data, no audio, no Windows changes

Settings live in `%LOCALAPPDATA%\Anada`. Set `ANADA_CONFIG_DIR` to an absolute folder
to run an isolated instance with its own settings.

## Tests

    cargo test --workspace --all-targets --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo fmt --all -- --check

The suite covers EQ, mixing, resampling, clock drift, queues and capture recovery; the
control model, settings migration, output selection, volume preservation, Quiet and
routing; MCP grants and revocation; and the UI through headless egui tests for layout
bounds, pointer drags, keyboard input, EQ selection, number entry and pill flow.

Five tests touch the real system and are ignored by default. Run them one at a time:

    cargo test --workspace --all-targets --locked -- --ignored --test-threads=1

They read real Windows devices, start and stop an isolated host, write temporary
settings, and write then remove a uniquely named startup entry.

## UI review

The demo mode renders the real interface with sample data. These variables control it:

| Variable | Effect |
| --- | --- |
| `ANADA_UI_WIDTH`, `ANADA_UI_HEIGHT` | Window size in logical pixels |
| `ANADA_UI_SCALE` | Pixels per point (0.75-2.0), for example 1.5 |
| `ANADA_UI_STRESS` | Long names, fractional volumes and many apps |
| `ANADA_UI_OUTPUTS` | A long output priority list, opened in Settings |
| `ANADA_UI_SCREENSHOT` | Save a PNG of the window to this path, then keep running |

Check UI changes at 100% and 150% scaling, with long names, fractional volumes,
different visible channel counts and height-only resizing.

## Packaging

See [packaging/README.md](../packaging/README.md) for the installer, the third-party
notices and the VAC setup helpers.

## Source layout

- `crates/anada-dsp`: stereo EQ, frequency response, mixing and output safety.
- `crates/anada-windows`: Windows endpoints, volume and mute, defaults, routing
  preferences and sessions.
- `crates/anada-app`: window, host, control model, IPC, persistence, tray, startup, MCP.
- `packaging`: NSIS installer, notice collection, user guide page and VAC helpers.
- `docs`: [user guide](GUIDE.md), [architecture](ARCHITECTURE.md) and [MCP](MCP.md).

# Anada

Anada is a Windows listening mixer written in Rust: four virtual playback channels on
Virtual Audio Cable lines, one physical listening output, per-app routing, and channel
and output EQ. These notes are for contributors and coding agents.

If an `AGENTS.local.md` file exists next to this one, read it too. It holds
maintainer-specific instructions and is not published.

## Read first

- [README.md](README.md) for what Anada does.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the host, window and audio path.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for build, test and UI review.
- [docs/GUIDE.md](docs/GUIDE.md) for how users set up and use Anada.

## Rules

- Windows endpoint volume and mute are the only volume controls. Do not add a second
  gain or volume system. Restarts, device changes, reconnects and binding edits must
  keep Windows volume as it is, including 100%. Only initial setup and the Quiet
  toggle lower volume.
- App routing follows Windows per-app preferences. Show a changed Windows default and
  offer an explicit repair; never force it back silently. Every drag action needs a
  non-drag alternative.
- Keep the audio pipeline small, with clear insertion points. No allocation, logging,
  file I/O or lock waits in real-time code.
- Run tests, strict Clippy and formatting before committing:
  `cargo test --workspace --all-targets --locked`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo fmt --all -- --check`.
- Stop the window and host (`anada.exe --stop-host`) before a build or test run that
  replaces the binaries. Stop the host after local audio tests.
- Never commit device-specific settings, measurements, logs, installers or driver
  packages. `work/`, `dist/` and `target/` are ignored for that reason.
- Contributions are licensed under GPL-3.0-or-later.

## UI checks

Keep the mixer as vertical channel columns with the selected EQ on the same page.
Window height must not change strip orientation or remove the EQ; scroll when space is
limited. Channel headers and strip clicks select their EQ directly.
Before a UI change is done, check fractional Windows percentages, long device and app
names, different visible channel counts, height-only resizing, and 100% and 150%
scaling, including resize transitions and real control bounds. For output priorities,
test real pointer drags in both directions on a long list with offline entries.
`anada.exe --ui-demo` with the variables in docs/DEVELOPMENT.md captures screenshots.

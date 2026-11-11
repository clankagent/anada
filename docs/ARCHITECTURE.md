# Architecture

## Overview

```text
Windows applications and Windows routing preferences
          |             |             |             |
       Gaming          Chat          Media          Aux
          +-------------+-------------+-------------+
          |  Four stereo VAC lines (playback/capture) |
          +--------------------+-----------------------+
                               |
                     Four separate audio inputs
                               |
       +-----------------------v--------------------------+
       | anada-host.exe                                  |
       |                                                  |
       | Each input: bounded queue -> drift/rate conversion|
       |             -> channel preamp + EQ                |
       |                                                  |
       | Sum four channels -> output preamp + EQ          |
       |                   -> output clamp -> render      |
       +-----------------------+--------------------------+
                               |
                     One shared-mode physical output

       anada.exe UI <--- local named pipe ---> host control
                         (commands and state, not audio)

       host control <--- windows-rs ---> Windows endpoint
                                        volume, sessions,
                                        devices and policy
```

Apps play into the four VAC playback endpoints. The host captures each line, applies
that channel's EQ, mixes the four channels, applies the output EQ for the current
listening device, and renders to that device in WASAPI shared mode.

## Processes

| Process | Responsibilities | Lifetime |
| --- | --- | --- |
| `anada-host.exe` | Audio engine, Windows integration, authoritative state, configuration, tray icon, Quiet state | Runs while you listen, independent of the window |
| `anada.exe` | Main window, channel strips, EQ editor, app assignment, settings, MCP session | Opens and closes without stopping audio |

The window attaches to the running host and starts it when needed. The host's control
pipe is created as the first pipe instance, so a second host cannot start. Closing or killing the window leaves audio running; **Exit
host** stops the mix. The host owns the tray icon on its own message loop, separate
from audio work. Optional sign-in startup is an entry in the per-user Windows Run key, so
Windows lists it under Startup apps and the user can also turn it off there.

`anada.exe` also has command-line modes that use the same pipe commands as the
window: `--probe` (read-only device and session inventory), `--snapshot`,
`--command-json`, `--stop-host`, `--mcp` and `--ui-demo`.

## Crates

- `anada-dsp`: validated stereo EQ, frequency response, mixing and finite-output safety.
- `anada-windows`: Windows endpoints, endpoint volume and mute, defaults, per-app
  routing preferences and audio sessions.
- `anada-app`: the window, the host, the control model, IPC, persistence, the tray,
  startup and MCP.

## Control model

The host holds the single source of truth. The window sends commands with the expected
configuration revision; the host validates them, calls Windows, and returns the
observed state. Reconnecting starts from a fresh snapshot. The host rescans Windows
devices and sessions when asked for state, at most every 750 ms.

Windows endpoint volume and mute are the only volume controls. Channel faders write
the VAC endpoints' Windows volume and the output fader writes the physical device's.
Anada keeps no second gain. EQ preamp is a separate, labelled DSP setting.

Quiet is host state. Turning it on stores the endpoint's current Windows volume and
lowers it to the configured level, never raising it. Turning it off restores the
stored volume. A manual or outside volume change, or a removed device, ends Quiet
without a restore. Switching the listening output restores the previous device and
ends its Quiet. Initial setup separately lowers new devices once: virtual channels to
at most -12 dB and the physical output to at most -24 dB (20% when no dB range is
available). Restarts, reconnects and binding edits never change Windows volume.

Gaming is the intended Windows default and Chat the communications default. Anada
shows when Windows differs and offers an explicit repair; it does not force them back.
App assignment writes the Windows per-application playback preference for every
process of an app. Anada observes live sessions separately, because an app can keep an
existing stream until it restarts playback.

## Output selection

The configuration stores stable endpoint IDs with readable names and an ordered
priority list. The host picks the highest connected listed device. A manual choice
applies at once and lasts until the next device connect or disconnect. A new device
enters the list just below the current choice; a known device keeps its rank when it
returns. When no listed device is available, the output stays silent and the window
says why. Changing the output keeps assignments, channel EQ and Windows volumes, and
loads that device's output EQ (flat for a new device).

## Audio path

Processing is stereo float32 at the output device's native rate. Each input has its own
bounded, preallocated queue (200 ms capacity, primed to 50 ms) and Rubato sinc
resampling whose ratio follows queue occupancy, bounded to +/-5000 ppm, so independent
device clocks never drift apart. The resampler works in preallocated 64-frame chunks.

On underrun the engine writes silence and counts it; queues never grow. Device changes
discard stale audio and build new streams outside the callbacks. EQ edits keep the
streams open: the control thread prepares new filter banks, passes them through a
bounded queue, the audio path crossfades for 20 ms, and the control thread frees the
old bank. Real-time code does not allocate, log, write files or wait on locks.

The output stage is a hard clamp that keeps samples finite and within full scale.
Meters report per-channel peaks after EQ and output peaks after the clamp, before
Windows volume. The spectrum is a 4096-sample Hann FFT shown as 128 logarithmic bands
from 20 Hz to 20 kHz, computed outside the audio callback.

## Configuration

Settings are versioned JSON in `%LOCALAPPDATA%\Anada` (`ANADA_CONFIG_DIR` overrides it).
They hold channel names, order, visibility and bindings, channel EQ, per-device output
EQ, the priority list, the quiet level and the EQ graph scale. Each save is atomic and
keeps the previous valid file as `settings.backup.json`. If the settings file is
invalid, Anada loads the backup and leaves the original untouched. Presets live
in `eq-presets.json` beside the settings and are written under a file lock.

## Local MCP access

`anada.exe --mcp` serves MCP over standard input and output. It never opens the window,
starts audio or enables access. The window owns the timed session and a loopback-only
broker on a random port with a protected discovery file. Every tool call is checked
against the session's grants, and tools that are not granted are not listed. See
[MCP access](MCP.md).

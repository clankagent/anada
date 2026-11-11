# Anada local MCP access

Anada can expose selected controls to an MCP-compatible local agent. The agent
connection uses the installed `anada.exe --mcp` command over standard input and
output. This command does not open the Anada window, start the audio host, or
enable access. The user must open Anada and enable a timed session first.

## Enable a session

In Anada, open **Settings > Agent access (MCP)**. Review the 25 permissions.
All are off on first use. Select only the permissions needed for the task. The
**Read-only** and **EQ assistant** buttons select permission sets; they do not
enable access. **Select all** is available, but grants every listed capability.
Choose 15, 30, 60, or 120 minutes (30 minutes is the default), then select
**Enable session**. The chosen permissions stay fixed for that session.

The agent setup panel can copy the server configuration for the current
installation. Add that configuration to the MCP settings for the agent you want
to use, then reconnect that agent after enabling a session. The configuration
contains the executable path and `--mcp` argument only. It has no password or
token. You can also use this shape, with the executable path copied from Anada:

```json
{
  "mcpServers": {
    "anada": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\Programs\\Anada\\anada.exe",
      "args": ["--mcp"]
    }
  }
}
```

While access is active, use **Stop MCP** in the Settings panel or the active
banner. Access also ends when its timer expires or the Anada window closes.
Restarting Anada does not resume a session. Saved permission choices remain for
the next review, but access always starts off. Stop the session before changing
its permissions.

## Permission groups

| Group | Permissions |
|---|---|
| Read | Status; Devices; Applications; Equalizers; Signal levels; Spectrum; Presets; Channel settings; Output settings |
| Equalizers and presets | Channel EQ; Output EQ; Save presets; Delete presets |
| Volume and mute | Channel volume; Output volume; Mute |
| Routing | Application routing; Select output; Output priorities |
| Channels | Channel names; Channel visibility; Channel bindings |
| System | Repair Windows defaults; Sign-in startup; Stop audio host |

The **Read-only** selection includes all nine Read permissions. **EQ assistant**
includes Status, Devices, Equalizers, Signal levels, Spectrum, Presets, Channel
EQ, and Output EQ. It does not include saving/deleting presets, Windows volume,
mute, routing, or host controls. The checkbox descriptions in Anada explain the
scope of each permission.

Examples of MCP tools include `read_eq`, `patch_channel_eq`,
`patch_output_eq`, `set_channel_volume`, `assign_applications`, and
`stop_host`. EQ tools can replace or patch a full EQ, edit one band, change
preamp or bypass, reset EQ, or recall a named preset. Preset read, save, and
delete have separate permissions. Volume and mute tools control Windows
endpoint settings. Routing and system tools can change persistent settings.
Applied configuration changes remain after the MCP session ends. Revoking MCP
access does not undo those changes.

The MCP interface exposes only the requested fields. For example, application
reads omit executable paths; EQ and endpoint levels are separate permissions;
signal levels do not include spectrum unless Spectrum is also granted. Tools
that are not allowed are not listed, and every call is checked again against
the current session grants.

## Local connection and limits

The Anada window owns a temporary broker on loopback. It uses a
random local port, a protected private discovery file, a per-user lock, and a
temporary capability token. The stdio adapter reads this local session; it
cannot create or extend one. The broker is not exposed to the LAN or a public network route.

If a change call times out, its result may be unknown. Check the current Anada
state before trying the change again. Do not automatically repeat a timed-out
mutation. MCP permissions limit the MCP tools; they do not sandbox any separate
operating-system or shell access an agent may have.

## MCP protocol support

The adapter supports the legacy stdio protocol revisions listed in its
discovery response and the modern `2026-07-28` discovery flow, and works with both
the legacy (1.32) and modern (2.3) official SDK clients. It uses bounded
newline-delimited JSON-RPC frames and keeps stdout for protocol messages.

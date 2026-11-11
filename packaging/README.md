# Windows packaging

The per-user NSIS installer installs `anada.exe`, `anada-host.exe`, the VAC setup
helpers below, third-party notices, Start-menu shortcuts (Anada, Set up Virtual Audio
Cable, Restore Virtual Audio Cable settings, Uninstall) and a registered uninstaller.
It leaves Windows audio defaults, sign-in startup and the VAC installation unchanged;
VAC itself is installed with its own installer.

## Build

1. Build the pinned GNU target: `cargo build --release --locked --workspace`.
2. Run `Collect-Notices.ps1` to regenerate `third-party/` from Cargo metadata and the
   local Cargo registry. It must report zero unresolved packages.
3. Run `Build-Guide.ps1` to generate `START-HERE.html` from `docs/GUIDE.md`.
4. Run `Build-Installer.ps1 -OutputFile dist/Anada-<version>-Setup.exe`. It expects the
   official NSIS 3.11 compiler at `work/tools/nsis/nsis-3.11/makensis.exe`, or pass
   `-MakeNsis <path>`.
5. Run `Test-Installer.ps1 -Installer <path>` on a dedicated Windows test machine.

Use the scripts under the machine's existing execution policy; do not change policy.
The installer test refuses to replace an existing registered installation.

`Build-Bundle.ps1` optionally builds an outer ZIP with the installer, the guide and the
VAC setup helpers. It can include the official VAC trial archive only when a verified,
unmodified copy is placed at `work/driver-qualification/vac471.zip`; check VAC's own
redistribution terms first. Never include a licensed VAC package, settings, endpoint
IDs, logs, credentials or development documents in a bundle. Record SHA-256 hashes
for each bundled file.

## VAC setup helpers

`Configure-VAC.cmd` (with `Configure-VAC.ps1`) replaces manual control-panel setup
after VAC is installed. It uses the documented VAC registry settings, saves changed
DWORD values before writing, mirrors the boot-time settings, discovers the installed
device through its exact VAC service identity, and restarts that device only. It
verifies a playback and recording endpoint for each standard Line 1-4 and preserves
additional cables. `Restore-VAC.cmd` restores the latest helper backup. Elevation uses
the normal UAC prompt; no execution policy, signing policy, test mode, reboot or other
driver's settings are changed.

`Test-VacSetup.ps1` runs the configure/restore logic in a unique HKCU subtree with
mocked device discovery, restart and elevation. It checks registry writes, backups,
idempotence, exact restoration, extra-cable preservation and refusal paths. It does
not test a real elevated driver restart.

## Installer safety

The installer refuses updates and removal while its own executables are running; the
user must exit the host first. An update keeps the sign-in startup entry. A logon task
from a version before 1.0.3 is replaced by the startup entry. Removal deletes the
startup entry when it points to this installation.
Interactive safety-check failures offer Retry and Cancel. Retry reruns the check
without reopening setup; Cancel closes the operation. Silent runs fail with exit code 2
without a dialog. A locked file during removal returns exit code 4 and keeps the
registration. Removal deletes only known application files, keeps user settings and
unrelated files, and never removes a shared VAC driver.

`Test-Installer.ps1` checks these paths: installation into a path with spaces, exact
binary and notice-file hashes, running-host refusal, startup-task migration, locked-file
refusal, and clean removal that preserves unrelated files and isolated settings.
The installer is unsigned.

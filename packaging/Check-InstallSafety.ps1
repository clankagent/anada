param([Parameter(Mandatory=$true)][string]$InstallDirectory, [switch]$Removing)
$ErrorActionPreference = 'Stop'
try {
    $folder = [IO.Path]::GetFullPath($InstallDirectory).TrimEnd('\')
    $registered = Get-ItemProperty -LiteralPath 'HKCU:/Software/Anada' -Name InstallLocation -ErrorAction SilentlyContinue
    if ($registered -and $registered.InstallLocation -and $registered.InstallLocation.TrimEnd('\') -ne $folder) {
        throw 'Anada is installed in another folder. Update that installation or remove it before changing folders.'
    }
    $marker = Join-Path $folder 'Anada-install.txt'
    if ((Test-Path -LiteralPath (Join-Path $folder 'anada.exe')) -or (Test-Path -LiteralPath (Join-Path $folder 'anada-host.exe')) -or (Test-Path -LiteralPath (Join-Path $folder 'Uninstall.exe'))) {
        if (!(Test-Path -LiteralPath $marker) -or (Get-Content -LiteralPath $marker -Raw).Trim() -ne 'Anada per-user preview installation v1') {
            throw 'The destination contains files not marked as this Anada installation. Choose a separate empty folder.'
        }
    }
    $owned = @((Join-Path $folder 'anada.exe'), (Join-Path $folder 'anada-host.exe'))
    foreach ($process in @(Get-CimInstance Win32_Process -Filter "Name='anada.exe' OR Name='anada-host.exe'")) {
        if (!$process.ExecutablePath) { throw 'Cannot verify an Anada process path. Close all Anada instances first.' }
        if ($owned -contains $process.ExecutablePath) { throw 'This installed Anada is running. Close its window and choose Exit host in its tray menu first.' }
    }
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $task = Get-ScheduledTask -TaskName ('Anada.Host.' + $sid) -TaskPath '\' -ErrorAction SilentlyContinue
    if ($task -and $task.Description -eq 'Anada per-user audio host; managed by Anada startup v1' -and @($task.Actions | Where-Object { $owned -contains $_.Execute }).Count) {
        # Versions before 1.0.3 started the host with a logon task. Keep that choice as
        # the standard Windows startup entry, then remove the task.
        if (!$Removing -and $task.Settings.Enabled -and @($task.Triggers).Count) {
            Set-ItemProperty -LiteralPath 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Run' -Name 'Anada' -Value ('"' + $owned[1] + '"')
        }
        Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath '\' -Confirm:$false
    }
    exit 0
} catch { Write-Output $_.Exception.Message; exit 1 }



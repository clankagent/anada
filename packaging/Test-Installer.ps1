param([string]$Installer = 'dist/Anada-Setup.exe')
$ErrorActionPreference = 'Stop'
if (!$PSScriptRoot) { $PSScriptRoot = Join-Path (Get-Location) 'packaging' }
$root = Split-Path $PSScriptRoot -Parent
$setup = [IO.Path]::GetFullPath((Join-Path $root $Installer))
$testRoot = Join-Path $root ('work/installer-test-' + [Guid]::NewGuid().ToString('N'))
$destination = Join-Path $testRoot 'Anada test with spaces'
$registry = 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Uninstall/Anada'
if ((Test-Path $registry) -or (Test-Path 'HKCU:/Software/Anada')) { throw 'Existing per-user registration; test refuses to replace it.' }
$shortcut = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs/Anada'
if (Test-Path -LiteralPath $shortcut) { throw 'Existing Start-menu folder; test refuses to replace it.' }
$run = 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Run'
if ($null -ne (Get-ItemProperty -LiteralPath $run).Anada) { throw 'Existing Anada startup entry; test refuses to replace it.' }
New-Item -ItemType Directory -Force $destination | Out-Null
$settings = Join-Path $testRoot 'isolated-settings'
New-Item -ItemType Directory $settings | Out-Null
Set-Content (Join-Path $settings 'preserve.txt') 'settings sentinel'
$originalConfig = $env:ANADA_CONFIG_DIR
$env:ANADA_CONFIG_DIR = $settings
$taskName = 'Anada.Host.' + [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$testTaskCreated = $false
$runCreated = $false
$hostStarted = $false
function Invoke-Anada([string]$Argument) {
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = Join-Path $destination 'anada.exe'
    $info.Arguments = $Argument
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $child = [Diagnostics.Process]::Start($info)
    $stdout = $child.StandardOutput.ReadToEndAsync()
    $stderr = $child.StandardError.ReadToEndAsync()
    if (!$child.WaitForExit(15000)) { $child.Kill(); throw "Installed CLI timed out: $Argument" }
    $result = @{ ExitCode=$child.ExitCode; Output=$stdout.GetAwaiter().GetResult(); Error=$stderr.GetAwaiter().GetResult() }
    $child.Dispose()
    if ($result.ExitCode -ne 0) { throw "Installed CLI failed: $Argument $($result.Error)" }
    return $result
}
try {
    $process = Start-Process -FilePath $setup -ArgumentList @('/S',"/D=$destination") -PassThru -Wait -WindowStyle Hidden
    if ($process.ExitCode -ne 0) { throw "Install failed: $($process.ExitCode)" }
    foreach ($name in @('anada.exe','anada-host.exe','Uninstall.exe','Anada-install.txt')) {
        if (!(Test-Path -LiteralPath (Join-Path $destination $name))) { throw "Missing installed file $name" }
    }
    foreach ($name in @('anada.exe','anada-host.exe')) {
        $expected = Get-FileHash -LiteralPath (Join-Path $root ('target/release/' + $name))
        $actual = Get-FileHash -LiteralPath (Join-Path $destination $name)
        if ($expected.Hash -ne $actual.Hash) { throw "Installed binary hash mismatch: $name" }
    }
    foreach ($name in @('Configure-VAC.cmd','Configure-VAC.ps1','Restore-VAC.cmd')) {
        $installed = Join-Path $destination $name
        if (!(Test-Path -LiteralPath $installed)) { throw "Missing installed VAC helper $name" }
        if ((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $name)).Hash -ne (Get-FileHash -LiteralPath $installed).Hash) { throw "Installed VAC helper mismatch: $name" }
    }
    $noticeRoot = Join-Path $PSScriptRoot 'third-party'
    $noticeFiles = @(Get-ChildItem -LiteralPath $noticeRoot -Recurse -File)
    foreach ($notice in $noticeFiles) {
        $relative = $notice.FullName.Substring($noticeRoot.Length).TrimStart('\')
        $installedNotice = Join-Path $destination ('third-party/' + $relative)
        if ((Get-FileHash -LiteralPath $notice.FullName).Hash -ne (Get-FileHash -LiteralPath $installedNotice).Hash) { throw "Notice mismatch: $relative" }
    }
    if ((Get-ItemProperty $registry).InstallLocation -ne $destination) { throw 'Wrong uninstall registration' }
    $probe = Invoke-Anada '--help'
    if ($probe.Output -notmatch 'Anada') { throw 'Installed help was not captured' }
    $uninstaller = Join-Path $destination 'Uninstall.exe'
    $snapshot = Invoke-Anada '--snapshot'
    $hostStarted = $true
    $state = $snapshot.Output | ConvertFrom-Json
    if ($state.audio_running) { throw 'Fresh isolated host unexpectedly started audio' }
    $hostProcess = Get-Process -Id $state.host_pid
    if ($hostProcess.Path -ne (Join-Path $destination 'anada-host.exe')) { throw 'Snapshot is not from installed host' }
    $process = Start-Process -FilePath $setup -ArgumentList @('/S',"/D=$destination") -PassThru -Wait -WindowStyle Hidden
    if ($process.ExitCode -ne 2) { throw "Running-host update was not refused: $($process.ExitCode)" }
    # _?= keeps this refusal check synchronous. It aborts before any deletion.
    $process = Start-Process -FilePath $uninstaller -ArgumentList @('/S',"_?=$destination") -PassThru -Wait -WindowStyle Hidden
    if ($process.ExitCode -ne 2) { throw "Running-host removal was not refused: $($process.ExitCode)" }
    Invoke-Anada '--stop-host' | Out-Null
    if (!$hostProcess.WaitForExit(5000)) { throw 'Installed isolated host did not exit' }
    $hostStarted = $false
    $hostCommand = '"' + (Join-Path $destination 'anada-host.exe') + '"'
    $startupCheck = 'run-entry-kept-by-update'
    if (!(Get-ScheduledTask -TaskName $taskName -TaskPath '\' -ErrorAction SilentlyContinue)) {
        # A logon task as versions before 1.0.3 registered it. An update must replace
        # it with the standard Windows startup entry.
        $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $action = New-ScheduledTaskAction -Execute (Join-Path $destination 'anada-host.exe')
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $sid
        $principal = New-ScheduledTaskPrincipal -UserId $sid -LogonType Interactive -RunLevel Limited
        Register-ScheduledTask -TaskName $taskName -TaskPath '\' -Action $action -Trigger $trigger -Principal $principal -Description 'Anada per-user audio host; managed by Anada startup v1' | Out-Null
        $testTaskCreated = $true
        $startupCheck = 'legacy-task-replaced-by-run-entry'
    } else {
        Set-ItemProperty -LiteralPath $run -Name 'Anada' -Value $hostCommand
    }
    $runCreated = $true
    $process = Start-Process -FilePath $setup -ArgumentList @('/S',"/D=$destination") -PassThru -Wait -WindowStyle Hidden
    if ($process.ExitCode -ne 0) { throw "Update with sign-in startup failed: $($process.ExitCode)" }
    if ($testTaskCreated) {
        if (Get-ScheduledTask -TaskName $taskName -TaskPath '\' -ErrorAction SilentlyContinue) { throw 'Update left the legacy logon task' }
        $testTaskCreated = $false
    }
    if ((Get-ItemProperty -LiteralPath $run).Anada -ne $hostCommand) { throw 'Update did not keep sign-in startup as a Run entry' }
    Set-Content (Join-Path $destination 'unrelated-keep.txt') 'preserve unrelated files'
    Set-Content (Join-Path $destination 'third-party/unrelated-keep.txt') 'preserve unrelated notice-folder file'
    $lock = [IO.File]::Open((Join-Path $destination 'anada.exe'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $process = Start-Process -FilePath $uninstaller -ArgumentList @('/S',"_?=$destination") -PassThru -Wait -WindowStyle Hidden
        if ($process.ExitCode -ne 4) { throw "Locked file removal did not fail safely: $($process.ExitCode)" }
        if (!(Test-Path $registry)) { throw 'Failed uninstall removed registration' }
        if (!(Test-Path -LiteralPath (Join-Path $destination 'Anada-install.txt'))) { throw 'Failed uninstall removed ownership marker' }
    } finally { $lock.Dispose() }
    # Normal removal copies itself to temp so its installed EXE can be deleted.
    $process = Start-Process -FilePath $uninstaller -ArgumentList @('/S') -PassThru -Wait -WindowStyle Hidden
    if ($process.ExitCode -ne 0) { throw "Uninstall launcher failed: $($process.ExitCode)" }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ((Test-Path $registry) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
    foreach ($name in @('anada.exe','anada-host.exe','Check-InstallSafety.ps1','Anada-install.txt','Uninstall.exe')) {
        if (Test-Path -LiteralPath (Join-Path $destination $name)) { throw "Uninstall left $name" }
    }
    foreach ($notice in $noticeFiles) {
        $relative = $notice.FullName.Substring($noticeRoot.Length).TrimStart('\')
        if (Test-Path -LiteralPath (Join-Path $destination ('third-party/' + $relative))) { throw "Uninstall left notice: $relative" }
    }
    if (!(Test-Path -LiteralPath (Join-Path $destination 'third-party/unrelated-keep.txt'))) { throw 'Uninstall deleted unrelated notice-folder data' }
    if (!(Test-Path -LiteralPath (Join-Path $destination 'unrelated-keep.txt'))) { throw 'Uninstall deleted unrelated data' }
    if (!(Test-Path -LiteralPath (Join-Path $settings 'preserve.txt'))) { throw 'Uninstall deleted settings' }
    if ((Test-Path $registry) -or (Test-Path 'HKCU:/Software/Anada')) { throw 'Uninstall left registration' }
    if (Test-Path -LiteralPath $shortcut) { throw 'Uninstall left shortcuts' }
    if ($null -ne (Get-ItemProperty -LiteralPath $run).Anada) { throw 'Uninstall left the sign-in startup entry' }
    $runCreated = $false
    [ordered]@{ installer=$setup; destination=$destination; installExit=0; noticeFilesVerified=$noticeFiles.Count; helpOutput=$probe.Output; runningHostRefusal='update-and-removal-code-2'; startup=$startupCheck; startupEntryRemoved=$true; lockedFileRefusal=4; uninstallExit=0; unrelatedFilePreserved=$true; isolatedSettingsPreserved=$true; registryRemoved=$true; shortcutsRemoved=$true } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $testRoot 'result.json') -Encoding UTF8
    Write-Output (Join-Path $testRoot 'result.json')
} finally {
    if ($hostStarted) { Invoke-Anada '--stop-host' | Out-Null }
    if ($testTaskCreated) { Unregister-ScheduledTask -TaskName $taskName -TaskPath '\' -Confirm:$false }
    if ($runCreated) { Remove-ItemProperty -LiteralPath $run -Name 'Anada' -ErrorAction SilentlyContinue }
    $env:ANADA_CONFIG_DIR = $originalConfig
}



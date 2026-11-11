param(
    [string]$BinaryDirectory = 'target/release',
    [string]$OutputFile = 'dist/Anada-Setup.exe',
    [string]$MakeNsis = 'work/tools/nsis/nsis-3.11/makensis.exe'
)
$ErrorActionPreference = 'Stop'
if (!$PSScriptRoot) { $PSScriptRoot = Join-Path (Get-Location) 'packaging' }
$root = Split-Path $PSScriptRoot -Parent
$bin = [IO.Path]::GetFullPath((Join-Path $root $BinaryDirectory))
$output = [IO.Path]::GetFullPath((Join-Path $root $OutputFile))
$compiler = [IO.Path]::GetFullPath((Join-Path $root $MakeNsis))
foreach ($name in @('anada.exe','anada-host.exe')) {
    if (!(Test-Path -LiteralPath (Join-Path $bin $name) -PathType Leaf)) { throw "Missing binary: $name" }
}
New-Item -ItemType Directory -Force (Split-Path $output -Parent) | Out-Null
$noticeRoot = Join-Path $PSScriptRoot 'third-party'
$noticeFiles = @(Get-ChildItem -LiteralPath $noticeRoot -Recurse -File | Sort-Object FullName)
if (!$noticeFiles.Count) { throw 'Third-party notices are missing. Run Collect-Notices.ps1 first.' }
$generated = Join-Path $root 'work/installer-generated'
New-Item -ItemType Directory -Force $generated | Out-Null
$noticeInclude = Join-Path $generated 'Notices.nsh'
$install = New-Object 'System.Collections.Generic.List[string]'
$delete = New-Object 'System.Collections.Generic.List[string]'
$directories = New-Object 'System.Collections.Generic.HashSet[string]'
$install.Add('!macro InstallNotices')
$delete.Add('!macro DeleteNotices')
foreach ($file in $noticeFiles) {
    $relative = $file.FullName.Substring($noticeRoot.Length).TrimStart('\')
    if ($relative.Contains('"') -or $relative.Contains('$')) { throw 'Unsupported notice filename' }
    $subdirectory = [IO.Path]::GetDirectoryName($relative)
    $install.Add('  SetOutPath "$INSTDIR\third-party\' + $subdirectory + '"')
    $install.Add('  File "' + $file.FullName + '"')
    $delete.Add('  Delete "$INSTDIR\third-party\' + $relative + '"')
    while ($subdirectory) {
        $directories.Add($subdirectory) | Out-Null
        $subdirectory = [IO.Path]::GetDirectoryName($subdirectory)
    }
}
$install.Add('!macroend')
$delete.Add('!macroend')
$removeDirs = @('!macro RemoveNoticeDirectories')
foreach ($directory in @($directories | Sort-Object { $_.Length } -Descending)) { $removeDirs += '  RMDir "$INSTDIR\third-party\' + $directory + '"' }
$removeDirs += '  RMDir "$INSTDIR\third-party"'
$removeDirs += '!macroend'
@($install.ToArray()) + @($delete.ToArray()) + $removeDirs | Set-Content -LiteralPath $noticeInclude -Encoding UTF8
Push-Location $PSScriptRoot
try {
    & $compiler '/V2' "/DNOTICES_INCLUDE=$noticeInclude" "/DBIN_DIR=$bin" "/DOUTPUT_FILE=$output" 'Anada.nsi'
    if ($LASTEXITCODE -ne 0) { throw "makensis failed: $LASTEXITCODE" }
} finally { Pop-Location }
Get-FileHash -LiteralPath $output -Algorithm SHA256





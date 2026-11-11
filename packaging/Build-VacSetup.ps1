$ErrorActionPreference = 'Stop'
$root = if ($PSScriptRoot) { Split-Path $PSScriptRoot -Parent } else { (Get-Location).Path }
$folder = Join-Path $root 'dist/Anada-VAC-Setup'
$files = @('Configure-VAC.cmd','Configure-VAC.ps1','Restore-VAC.cmd','VAC-SETUP-READ-ME.txt')
New-Item -ItemType Directory -Path $folder -Force | Out-Null
foreach ($file in Get-ChildItem -LiteralPath $folder -Force) {
    if ($file.PSIsContainer -or $file.Name -notin ($files + @('SHA256SUMS.txt'))) { throw "Unexpected helper package content: $($file.Name)" }
}
foreach ($name in $files) { Copy-Item -LiteralPath (Join-Path $root "packaging/$name") -Destination $folder -Force }
$hashes = foreach ($name in $files) { (Get-FileHash -LiteralPath (Join-Path $folder $name)).Hash.ToLowerInvariant() + "  $name" }
[IO.File]::WriteAllLines((Join-Path $folder 'SHA256SUMS.txt'), $hashes, [Text.UTF8Encoding]::new($false))
$zip = Join-Path $root 'dist/Anada-VAC-Setup.zip'
Compress-Archive -LiteralPath $folder -DestinationPath $zip -Force
Get-FileHash -LiteralPath $zip

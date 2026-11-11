param([string]$Version = '1.0.2',[string]$BundleSuffix='-Automatic-VAC')
$ErrorActionPreference = 'Stop'
$root = if ($PSScriptRoot) { Split-Path $PSScriptRoot -Parent } else { (Get-Location).Path }
$name = "Anada-$Version-Windows-x64$BundleSuffix"
$folder = Join-Path $root "dist/$name"
$setupName = "Anada-$Version-Setup.exe"
$setup = Join-Path $root "dist/$setupName"
$trial = Join-Path $root 'work/driver-qualification/vac471.zip'
if (!(Test-Path -LiteralPath $setup -PathType Leaf)) { throw 'Build and test the installer first.' }
if ((Get-FileHash -LiteralPath $trial -Algorithm SHA256).Hash -ne '380338E5CF4E9186C7E5FC44DB1509761369E61EAFBB650B139A58F7C3ECF2A6') {
    throw 'VAC trial archive differs from the verified original. Do not repackage it.'
}
New-Item -ItemType Directory -Force $folder | Out-Null
$helpers = @('Configure-VAC.cmd','Configure-VAC.ps1','Restore-VAC.cmd')
$allowed = @($setupName, 'START-HERE.html', 'VAC-4.71-Trial.zip', 'SHA256SUMS.txt') + $helpers
foreach ($file in Get-ChildItem -LiteralPath $folder -Force) {
    if ($file.PSIsContainer -or $file.Name -notin $allowed) { throw "Unexpected bundle content: $($file.Name)" }
}
Copy-Item -LiteralPath $setup -Destination (Join-Path $folder $setupName) -Force
Copy-Item -LiteralPath (Join-Path $root 'packaging/START-HERE.html') -Destination $folder -Force
Copy-Item -LiteralPath $trial -Destination (Join-Path $folder 'VAC-4.71-Trial.zip') -Force
foreach ($helper in $helpers) { Copy-Item -LiteralPath (Join-Path $root "packaging/$helper") -Destination $folder -Force }
$hashes = foreach ($fileName in (@($setupName, 'START-HERE.html', 'VAC-4.71-Trial.zip') + $helpers)) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $folder $fileName) -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $fileName"
}
[IO.File]::WriteAllLines((Join-Path $folder 'SHA256SUMS.txt'), $hashes, [Text.UTF8Encoding]::new($false))
$archive = Join-Path $root "dist/$name.zip"
Compress-Archive -LiteralPath $folder -DestinationPath $archive -Force
Get-FileHash -LiteralPath $archive -Algorithm SHA256

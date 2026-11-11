param([string]$Metadata)
$ErrorActionPreference='Stop'
$root=if($PSScriptRoot){Split-Path $PSScriptRoot -Parent}else{(Get-Location).Path}
$out=Join-Path $root 'packaging/third-party'
$templates=Join-Path $root 'packaging/license-templates'
New-Item -ItemType Directory -Force $out | Out-Null
$metadataPath=if($Metadata){Join-Path $root $Metadata}else{$null}
if($metadataPath -and (Test-Path $metadataPath)){$m=Get-Content $metadataPath -Raw -Encoding UTF8 | ConvertFrom-Json} else {
 Push-Location $root
 try {$json=& (Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe') metadata --offline --locked --format-version 1 --filter-platform x86_64-pc-windows-gnu;if($LASTEXITCODE -ne 0){throw 'Cargo metadata failed'};$m=($json -join "`n")|ConvertFrom-Json} finally {Pop-Location}
}
# Notices cover what the Windows installer ships: the x86_64-pc-windows-gnu graph only.
$manifest=@(); $failures=@()
function Copy-Notice($source,$target) {
 New-Item -ItemType Directory -Force (Split-Path $target -Parent)|Out-Null
 Copy-Item -LiteralPath $source -Destination $target
}
function Normalize-Text([string]$text) {
 return (($text.ToLowerInvariant() -replace '\s','') -replace '[\u201c\u201d]','"')
}
# Test content, never a filename or an SPDX mention. Unknown texts remain unresolved.
# MIT uses the complete permission/condition/disclaimer body, ignoring whitespace.
# Apache requires the complete terms (the optional application appendix may vary).
# Other supported texts require their operative clauses and final disclaimer.
function Test-LicenseText([string]$id,[string]$text) {
 $t=Normalize-Text $text
 switch -Exact ($id) {
  'MIT' {
   $body=Normalize-Text (Get-Content (Join-Path $templates 'MIT.txt') -Raw -Encoding UTF8)
   return $t.Contains($body.Replace('mitlicense',''))
  }
  'Apache-2.0' {
   $body=Normalize-Text (Get-Content (Join-Path $templates 'Apache-2.0.txt') -Raw -Encoding UTF8)
   $end='endoftermsandconditions'
   $body=$body.Substring(0,$body.IndexOf($end)+$end.Length)
   return $t.Contains($body)
  }
  'BSL-1.0' {$parts=@('Boost Software License - Version 1.0','Permission is hereby granted, free of charge','The copyright notices in the Software and this entire statement','machine-executable object code','THE SOFTWARE IS PROVIDED "AS IS"','OTHER DEALINGS IN THE SOFTWARE')}
  'Zlib' {$parts=@("provided 'as-is'",'Permission is granted to anyone','The origin of this software must not be misrepresented','Altered source versions must be plainly marked','This notice may not be removed or altered')}
  'ISC' {$parts=@('Permission to use, copy, modify, and/or distribute','THE SOFTWARE IS PROVIDED "AS IS"','IN NO EVENT SHALL','USE OR PERFORMANCE OF THIS SOFTWARE')}
  'Unicode-3.0' {$parts=@('UNICODE LICENSE V3','Permission is hereby granted, free of charge','permission notice appear with all copies','THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS"','prior written authorization of the copyright holder')}
  'OFL-1.1' {$parts=@('SIL OPEN FONT LICENSE Version 1.1','PERMISSION & CONDITIONS','TERMINATION','DISCLAIMER','FONT SOFTWARE IS PROVIDED "AS IS"','OTHER DEALINGS IN THE FONT SOFTWARE')}
  'Ubuntu-font-1.0' {$parts=@('UBUNTU FONT LICENCE Version 1.0','PERMISSION & CONDITIONS','TERMINATION','DISCLAIMER','FONT SOFTWARE IS PROVIDED "AS IS"','OTHER DEALINGS IN THE FONT SOFTWARE')}
  default {return $false}
 }
 foreach($s in $parts){if(!$t.Contains((Normalize-Text $s))){return $false}}
 return $true
}
# Recursive descent: AND binds tighter than OR; slash is Cargo's legacy OR.
# WITH stays an atomic obligation: a base license alone cannot cover its exception.
function Read-LicenseAtom($state) {
 if($state.i -ge $state.tokens.Count){throw 'Expected license'}
 $token=$state.tokens[$state.i]; $state.i++
 if($token -eq '(') {
  $node=Read-LicenseOr $state
  if($state.i -ge $state.tokens.Count -or $state.tokens[$state.i] -ne ')'){throw 'Missing closing parenthesis'}
  $state.i++; return $node
 }
 if($token -in @('AND','OR','WITH',')')){throw 'Expected license identifier'}
 if($state.i -lt $state.tokens.Count -and $state.tokens[$state.i] -eq 'WITH'){
  $state.i++
  if($state.i -ge $state.tokens.Count){throw 'Missing license exception'}
  $token+=' WITH '+$state.tokens[$state.i]; $state.i++
 }
 return @{op='license';id=$token}
}
function Read-LicenseAnd($state) {
 $node=Read-LicenseAtom $state
 while($state.i -lt $state.tokens.Count -and $state.tokens[$state.i] -eq 'AND'){
  $state.i++; $node=@{op='AND';left=$node;right=(Read-LicenseAtom $state)}
 }
 return $node
}
function Read-LicenseOr($state) {
 $node=Read-LicenseAnd $state
 while($state.i -lt $state.tokens.Count -and $state.tokens[$state.i] -eq 'OR'){
  $state.i++; $node=@{op='OR';left=$node;right=(Read-LicenseAnd $state)}
 }
 return $node
}
function Get-LicensePlan($node,$coverage,[bool]$allowReconstruction) {
 if($node.op -eq 'license'){
  if($coverage.ContainsKey($node.id)){return @{ok=$true;chosen=@($node.id);missing=@()}}
  if($allowReconstruction -and $node.id -in @('MIT','Apache-2.0')){
   return @{ok=$true;chosen=@($node.id);missing=@($node.id)}
  }
  return @{ok=$false;chosen=@();missing=@()}
 }
 $a=Get-LicensePlan $node.left $coverage $allowReconstruction
 $b=Get-LicensePlan $node.right $coverage $allowReconstruction
 if($node.op -eq 'AND'){return @{ok=($a.ok -and $b.ok);chosen=@($a.chosen+$b.chosen|Select-Object -Unique);missing=@($a.missing+$b.missing|Select-Object -Unique)}}
 if($a.ok -and (!$b.ok -or $a.missing.Count -le $b.missing.Count)){return $a}
 return $b
}
foreach($p in $m.packages | Sort-Object name,version) {
 # Cargo workspace membership, not its license or a missing registry source,
 # identifies first-party packages (including inherited GPL workspace licenses).
 if($p.id -in $m.workspace_members){continue}
 $entry=[ordered]@{name=$p.name;version=$p.version;license=$p.license;repository=$p.repository;source=$p.source;revision=$null;noticeFiles=@();upstreamUrls=@();provenanceNote=$null;selectedLicenses=@();licenseEvidence=@{};unresolved=$null}
 $dir=Split-Path $p.manifest_path
 $prefix='crates/'+$p.name+'-'+$p.version
 $dest=Join-Path $out $prefix
 # Rebuild each generated directory so a previous download/substitution cannot
 # silently satisfy coverage or remain in the installer after regeneration.
 $expected=[IO.Path]::GetFullPath((Join-Path $out 'crates'))+[IO.Path]::DirectorySeparatorChar
 if(!([IO.Path]::GetFullPath($dest).StartsWith($expected,[StringComparison]::OrdinalIgnoreCase))){throw 'Unsafe notice destination'}
 if(Test-Path $dest){Remove-Item -LiteralPath $dest -Recurse -Force}
 $texts=@()
 $files=@(Get-ChildItem $dir -Recurse -File | Where-Object {
  $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT|AUTHORS)([._-]|$)' -or
  $_.Extension -eq '.txt' -or ($p.license_file -and $_.FullName -eq (Join-Path $dir $p.license_file))
 })
 foreach($f in $files){
  $rel=$f.FullName.Substring($dir.Length+1).Replace('\','/')
  $text=Get-Content $f.FullName -Raw -Encoding UTF8
  # Include supplemental notices (including embedded font attributions).
  Copy-Notice $f.FullName (Join-Path $dest $rel)
  $entry.noticeFiles+=($prefix+'/'+$rel)
  $texts+=@{path=($prefix+'/'+$rel);text=$text;primary=($f.DirectoryName -eq $dir)}
 }
 $vcs=Join-Path $dir '.cargo_vcs_info.json'
 if(Test-Path $vcs){$entry.revision=(Get-Content $vcs -Raw|ConvertFrom-Json).git.sha1}
 # Existing repository cache is usable offline ONLY at the pinned crate revision.
 # In particular, do not substitute a sibling egui crate's different revision.
 if($entry.revision -and $p.repository -match 'github.com/([^/]+/[^/]+)'){
  $repo=$Matches[1] -replace '\.git$',''
  $cache=Join-Path $root ('work/notice-cache/'+$repo.Replace('/','_')+'_'+$entry.revision)
  if(Test-Path $cache){
   foreach($f in @(Get-ChildItem $cache -Recurse -File | Where-Object {$_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT|AUTHORS)([._-]|$)'})){
    $rel=$f.FullName.Substring($cache.Length+1).Replace('\','/')
    $path=$prefix+'/upstream/'+$rel
    Copy-Notice $f.FullName (Join-Path $dest ('upstream/'+$rel))
    $entry.noticeFiles+=$path
    $entry.upstreamUrls+=('https://raw.githubusercontent.com/'+$repo+'/'+$entry.revision+'/'+$rel)
    $texts+=@{path=$path;text=(Get-Content $f.FullName -Raw -Encoding UTF8);primary=($f.DirectoryName -eq $cache)}
   }
   if($entry.upstreamUrls.Count){$entry.provenanceNote='License files from the existing offline repository cache at the exact published crate revision; upstream URLs are provenance only and were not fetched.'}
  }
 }
 try {
  if(!$p.license){throw 'No declared SPDX license'}
  $expr=$p.license.Replace('/',' OR ')
  $tokens=@([regex]::Matches($expr,'\(|\)|[A-Za-z0-9.+-]+')|ForEach-Object {$_.Value})
  if(($expr -replace '\(|\)|[A-Za-z0-9.+-]+|\s','') -ne ''){throw 'Unsupported SPDX syntax'}
  $state=@{tokens=$tokens;i=0}; $ast=Read-LicenseOr $state
  if($state.i -ne $tokens.Count){throw 'Unexpected SPDX token'}
  $coverage=@{}
  foreach($id in ($tokens|Where-Object {$_ -notin @('(',')','OR','AND','WITH')}|Select-Object -Unique)){
   foreach($t in $texts){
    # A bundled font/vendor MIT text is not the crate's own permission.
    if($id -in @('MIT','Apache-2.0','BSL-1.0','Zlib','ISC') -and !$t.primary){continue}
    if(Test-LicenseText $id $t.text){
     if(!$coverage.ContainsKey($id)){$coverage[$id]=@()}
     $coverage[$id]+=$t.path
    }
   }
  }
  $before=Get-LicensePlan $ast $coverage $false
  $entry['resolvedBeforeReconstruction']=$before.ok
  $plan=Get-LicensePlan $ast $coverage $true
  if(!$plan.ok){throw ('Missing actual license text for expression: '+$p.license)}
  foreach($id in $plan.missing){
   $authors=@($p.authors|Where-Object {$_})
   if(!$authors.Count){throw ('Cannot reconstruct '+$id+': Cargo.toml has no authors')}
   $rel='reconstructed/LICENSE-'+$id+'.txt'
   $header=@(
    'Reconstructed license notice - not an upstream license file.'
    ('Package: '+$p.name+' '+$p.version)
    ('Declared SPDX license (published Cargo.toml): '+$p.license)
    ('Chosen option: '+$id)
    'Authors (verbatim Cargo.toml authors, as reported by cargo metadata):'
    ($authors -join "`r`n")
    'No copyright year is inferred. Authors are attribution metadata, not a verified exhaustive list of copyright holders.'
    'Canonical license text follows; reconstructed from the declared SPDX license.'
    ''
   ) -join "`r`n"
   New-Item -ItemType Directory -Force (Join-Path $dest 'reconstructed')|Out-Null
   $text=$header+"`r`n"+(Get-Content (Join-Path $templates ($id+'.txt')) -Raw -Encoding UTF8)
   if(!(Test-LicenseText $id $text)){throw ('Invalid canonical template: '+$id)}
   $text|Set-Content (Join-Path $dest $rel) -Encoding UTF8
   $entry.noticeFiles+=($prefix+'/'+$rel)
   $coverage[$id]=@($prefix+'/'+$rel)
  }
  if($plan.missing.Count){$entry.provenanceNote='Missing crate license text reconstructed offline from this pinned package SPDX declaration and Cargo.toml authors; no different-revision license substitution.'}
  $entry.selectedLicenses=@($plan.chosen)
  foreach($id in $plan.chosen){$entry.licenseEvidence[$id]=@($coverage[$id])}
 } catch {
  $entry.unresolved=$_.Exception.Message
  $failures+=($p.name+' '+$p.version+': '+$entry.unresolved)
 }
 $manifest+=[pscustomobject]$entry
}
$rust=Join-Path $env:USERPROFILE '.rustup/toolchains/1.98.1-x86_64-pc-windows-gnu/share/doc/rust'
Copy-Notice (Join-Path $rust 'COPYRIGHT-library.html') (Join-Path $out 'rust/COPYRIGHT-library.html')
Get-ChildItem (Join-Path $rust 'licenses') -Recurse -File|ForEach-Object {Copy-Notice $_.FullName (Join-Path $out ('rust/licenses/'+$_.FullName.Substring((Join-Path $rust 'licenses').Length+1)))}
$runtime=Join-Path $env:USERPROFILE '.rustup/toolchains/1.98.1-x86_64-pc-windows-gnu/share/doc'
foreach($name in @('COPYING3','COPYING.RUNTIME')) {Copy-Notice (Join-Path $runtime $name) (Join-Path $out ('rust-gnu-runtime/'+$name))}
foreach($kind in @('crt','headers','gcc','libgcc','winpthreads','libwinpthread')) {
 $folder='C:/msys64/mingw64/share/licenses/'+$kind
 if(Test-Path $folder){Get-ChildItem $folder -Recurse -File|ForEach-Object {Copy-Notice $_.FullName (Join-Path $out ('mingw/'+$kind+'/'+$_.FullName.Substring($folder.Length+1)))}}
}
$manifest|ConvertTo-Json -Depth 12|Set-Content (Join-Path $out 'manifest.json') -Encoding utf8
# Keep diagnostics inside the generated notice folder; never alter unrelated work/.
 ConvertTo-Json -InputObject @($failures) -Compress|Set-Content (Join-Path $out 'unresolved.json') -Encoding utf8
$beforeCount=@($manifest|Where-Object {!$_.resolvedBeforeReconstruction}).Count
Write-Output ('Packages: '+$manifest.Count+'; unresolved before reconstruction: '+$beforeCount+'; unresolved: '+$failures.Count+'; files: '+@(Get-ChildItem $out -Recurse -File).Count)
if($failures.Count){$failures|Write-Output;exit 1}

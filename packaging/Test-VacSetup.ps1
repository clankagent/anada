$ErrorActionPreference='Stop'
$homeDir=Join-Path (Get-Location) 'work/vac-script-tests'
$production=Resolve-Path 'packaging/Configure-VAC.ps1'
$source=[IO.File]::ReadAllText($production)
$id=[guid]::NewGuid().ToString('N')
$base="HKCU:\Software\AnadaVacQA\$id"
$roots=@("$base\Software","$base\Parameters")
$store=Join-Path $homeDir "results-$id"
New-Item -ItemType Directory $store|Out-Null
$ast=[System.Management.Automation.Language.Parser]::ParseInput($source,[ref]$null,[ref]$null)
$replace=@()
foreach($name in @('Discover-VAC','Restart-VAC')){
 $node=$ast.Find({param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true)
 $body=if($name -eq 'Discover-VAC'){'function Discover-VAC { return [pscustomobject]@{Status="OK";InstanceId="MOCK-VAC"} }'}else{"function Restart-VAC(`$device,[bool]`$RequireStandardPairs=`$true) { Add-Content -LiteralPath '$store/restarts.txt' -Value 'restart'; if(`$RequireStandardPairs -and !(Test-StandardPairs)){throw 'mock pair verification failed'} }"}
 $replace+=@{Start=$node.Extent.StartOffset;Length=$node.Extent.EndOffset-$node.Extent.StartOffset;Text=$body}
}
foreach($item in ($replace|Sort-Object Start -Descending)){$source=$source.Remove($item.Start,$item.Length).Insert($item.Start,$item.Text)}
$source=$source -replace '(?m)^\$roots=.*$',("`$roots=@('$($roots[0])','$($roots[1])')")
$source=$source -replace '(?m)^\$store=.*$',("`$store='$store'")
$source=$source -replace '(?m)^  \$admin=.*$','  $admin=$true # TEST ONLY: never elevate'
if($source.Contains('HKLM:') -or $source.Contains('pnputil.exe') -or !$source.Contains('$admin=$true # TEST ONLY: never elevate')){throw 'Test isolation replacement failed. Refusing to run.'}
$mocks=@'
function Get-PnpDevice {
 param($Class,[switch]$PresentOnly,$InstanceId,$ErrorAction)
 for($n=1;$n -le 4;$n++){foreach($flow in @(0,1)){
 [pscustomobject]@{Status='OK';FriendlyName="Line $n (Virtual Audio Cable)";InstanceId="SWD\MMDEVAPI\{0.0.$flow.00000000}.{$n-$flow}"}
 }}
}
function Get-Process { param($Name,$ErrorAction) if($env:VAC_QA_HOST -eq '1'){[pscustomobject]@{ProcessName='anada-host'}} }
'@
# Preserve the real script's param statement as the first executable statement.
$firstBreak=$source.IndexOf("`n")
$source=$source.Insert($firstBreak+1,$mocks+"`n")
$runner=Join-Path $store 'Isolated-Configure-VAC.ps1'
[IO.File]::WriteAllText($runner,$source)
function Assert($condition,$message){if(!$condition){throw "ASSERT: $message"}}
function Put($path,$name,$value){if(!(Test-Path $path)){New-Item -Path $path -Force|Out-Null};New-ItemProperty -LiteralPath $path -Name $name -PropertyType DWord -Value $value -Force|Out-Null}
function Run($mode,$expected=0){& powershell.exe -NoProfile -Command "& ([scriptblock]::Create([IO.File]::ReadAllText('$runner'))) -Mode $mode";Assert ($LASTEXITCODE -eq $expected) "$mode exit=$LASTEXITCODE expected=$expected"}
function Count-Backups {@(Get-ChildItem $store -Directory -Filter 'backup-*').Count}
function Count-Restarts {if(Test-Path "$store/restarts.txt"){@(Get-Content "$store/restarts.txt").Count}else{0}}
function Snapshot {foreach($root in $roots){if(Test-Path $root){reg query ($root -replace '^HKCU:','HKCU') /s}}}
try{
 foreach($root in $roots){Put $root 'Number of cables' 1;Put "$root\Cable 1" 'Minimum number of channels' 0;Put "$root\Cable 1" 'Maximum number of channels' 8;Put "$root\Cable 1" 'Volume control' 0;Put $root 'Unrelated' 99}
 $initial=Snapshot|Out-String
 Run Configure
 foreach($root in $roots){Assert ((Get-ItemProperty $root).'Number of cables' -eq 4) 'count=4';for($n=1;$n -le 4;$n++){ $v=Get-ItemProperty "$root\Cable $n";Assert ($v.'Minimum number of channels' -eq 2 -and $v.'Maximum number of channels' -eq 2 -and $v.'Volume control' -eq 1) "cable $n stereo+volume"}}
 Assert ((Count-Backups) -eq 1 -and (Count-Restarts) -eq 1) 'first configure backup/restart'
 $folder=(Get-Content "$store/latest.txt").Trim();$backup=Get-Content "$store/$folder/values.json" -Raw|ConvertFrom-Json
 Assert (@($backup.Values).Count -eq 26) '26 mirrored values backed up'
 Assert (@($backup.Values|Where-Object {!$_.Exists}).Count -eq 18) '18 previously absent cable values recorded'
 Run Configure
 Assert ((Count-Backups) -eq 1 -and (Count-Restarts) -eq 1) 'idempotent skips backup/restart'
 Run Restore
 # Empty newly created keys are harmless; compare values and existence explicitly.
 foreach($root in $roots){$v=Get-ItemProperty $root;Assert ($v.'Number of cables' -eq 1 -and $v.Unrelated -eq 99) 'count/unrelated restored';$c=Get-ItemProperty "$root\Cable 1";Assert ($c.'Minimum number of channels' -eq 0 -and $c.'Maximum number of channels' -eq 8 -and $c.'Volume control' -eq 0) 'original 0/8/0 restored';for($n=2;$n -le 4;$n++){Assert ((Get-Item "$root\Cable $n").GetValueNames().Count -eq 0) 'absent values removed'}}
 Put $roots[0] 'Number of cables' 5;Put $roots[1] 'Number of cables' 7
 Run Configure
 foreach($root in $roots){Assert ((Get-ItemProperty $root).'Number of cables' -eq 7) 'extra count preserved and mirrored'}
 $beforeHost=Snapshot|Out-String;$restartCount=Count-Restarts;$backupCount=Count-Backups
 $env:VAC_QA_HOST='1';Run Configure 1;$env:VAC_QA_HOST=$null
 Assert ((Snapshot|Out-String) -eq $beforeHost -and (Count-Restarts) -eq $restartCount -and (Count-Backups) -eq $backupCount) 'active host no mutation'
 $folder=(Get-Content "$store/latest.txt").Trim();$path="$store/$folder/values.json";$valid=Get-Content $path -Raw;$bad=$valid|ConvertFrom-Json;$bad.Values=@($bad.Values[0],$bad.Values[0]);$bad|ConvertTo-Json -Depth 8|Set-Content $path
 Run Restore 1
 Assert ((Snapshot|Out-String) -eq $beforeHost -and (Count-Restarts) -eq $restartCount) 'duplicate backup rejected before writes'
 Set-Content $path $valid
 'PASS: configure, backup existence, idempotence, exact restore, unrelated preservation, 5/7 mirror, active host refusal, duplicate-backup rejection.'|Tee-Object "$store/result.txt"
 "Results: $store"
}finally{
 $env:VAC_QA_HOST=$null
 # Only this test's GUID subtree, created above, is removed.
 if($base -match '^HKCU:\\Software\\AnadaVacQA\\[a-f0-9]{32}$'){Remove-Item -LiteralPath $base -Recurse -Force -ErrorAction SilentlyContinue}
}

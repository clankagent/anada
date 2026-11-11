param([ValidateSet('Check','Configure','Restore')][string]$Mode='Configure',[switch]$Reapply)
$ErrorActionPreference='Stop'
$service='VirtualAudioCable_83ed7f0e'
$roots=@('HKLM:\SOFTWARE\EuMus Design\Virtual Audio Cable\4',"HKLM:\SYSTEM\CurrentControlSet\Services\$service\Parameters")
$store=Join-Path $env:ProgramData 'Anada\VACSetup'
function Read-Value($path,$name) {
  if(Test-Path -LiteralPath $path) {
    $key=Get-Item -LiteralPath $path
    if($key.GetValueNames() -contains $name) { return @{Exists=$true;Kind=$key.GetValueKind($name).ToString();Value=$key.GetValue($name)} }
  }
  return @{Exists=$false;Kind=$null;Value=$null}
}
function Value-Path($entry) { Join-Path $roots[[int]$entry.Root] ([string]$entry.Subkey) }
function Discover-VAC {
  if(!(Test-Path "HKLM:\SYSTEM\CurrentControlSet\Services\$service")) { throw 'Virtual Audio Cable is not installed. Install VAC 4.71 first.' }
  $found=@(Get-PnpDevice -Class Media -PresentOnly | Where-Object {
    (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_Service -ErrorAction SilentlyContinue).Data -eq $service
  })
  if($found.Count -ne 1) { throw "Expected one installed VAC device; found $($found.Count). No settings changed." }
  return $found[0]
}
function Verify-Values($entries) {
  foreach($entry in $entries) {
    $actual=Read-Value (Value-Path $entry) $entry.Name
    if($actual.Exists -ne [bool]$entry.Exists -or ($entry.Exists -and ($actual.Kind -ne $entry.Kind -or [string]$actual.Value -ne [string]$entry.Value))) { throw "Registry verification failed: $($entry.Subkey) / $($entry.Name)." }
  }
}
function Test-StandardPairs {
  $endpoints=@(Get-PnpDevice -Class AudioEndpoint -PresentOnly | Where-Object {$_.Status -eq 'OK'})
  for($n=1;$n -le 4;$n++) {
    $pair=@($endpoints | Where-Object {$_.FriendlyName -eq "Line $n (Virtual Audio Cable)"})
    if($pair.Count -ne 2 -or @($pair|Where-Object {$_.InstanceId -match '\{0\.0\.0\.'}).Count -ne 1 -or @($pair|Where-Object {$_.InstanceId -match '\{0\.0\.1\.'}).Count -ne 1){return $false}
  }
  return $true
}
function Restart-VAC($device,[bool]$RequireStandardPairs=$true) {
  & "$env:windir\System32\pnputil.exe" /restart-device $device.InstanceId | Out-Host
  $result=$LASTEXITCODE
  if($result -eq 3010) { throw 'VAC requests a Windows restart. Settings are saved, but effective configuration is not verified. Restart Windows when convenient, then run Check.' }
  if($result -ne 0) { throw "VAC restart failed (exit $result). Settings are saved, but effective configuration is not verified." }
  for($i=0;$i -lt 20;$i++) {
    Start-Sleep -Milliseconds 500
    $current=Get-PnpDevice -InstanceId $device.InstanceId -ErrorAction SilentlyContinue
    if($current.Status -eq 'OK' -and (!$RequireStandardPairs -or (Test-StandardPairs))) { return }
  }
  if($current.Status -ne 'OK') { throw 'VAC did not return to the OK state after restart.' }
  throw 'VAC returned, but each standard Line 1-4 playback/recording pair was not verified. Renamed endpoints require manual checking in Windows Sound.'
}
try {
  $device=Discover-VAC
  $target=@()
  $desired=4
  foreach($root in $roots){
    $count=Read-Value $root 'Number of cables'
    if($count.Exists){
      if($count.Kind -ne 'DWord' -or [long]$count.Value -lt 0 -or [long]$count.Value -gt 256){throw 'Existing VAC cable count is invalid. No settings changed.'}
      $desired=[Math]::Max($desired,[int]$count.Value)
    }
  }
  for($r=0;$r -lt 2;$r++) {
    $target+=@{Root=$r;Subkey='';Name='Number of cables';Exists=$true;Kind='DWord';Value=$desired}
    for($n=1;$n -le 4;$n++) {
      foreach($name in @('Minimum number of channels','Maximum number of channels','Volume control')) {
        $value=2;if($name -eq 'Volume control'){$value=1}
        $target+=@{Root=$r;Subkey="Cable $n";Name=$name;Exists=$true;Kind='DWord';Value=$value}
      }
    }
  }
  if($Mode -eq 'Check') {
    Verify-Values $target
    if($device.Status -ne 'OK' -or !(Test-StandardPairs)) { throw 'Registry settings match, but each standard active VAC Line 1-4 playback/recording pair is not verified. Check renamed devices in Windows Sound.' }
    Write-Host 'VAC settings match: four stereo cable pairs with Windows volume control. Eight standard endpoints are present.'
    exit 0
  }
  if(Get-Process -Name 'anada-host','anada' -ErrorAction SilentlyContinue) { throw 'Close Anada, including Exit host from its tray menu, then run this setup again.' }
  $changes=@();$before=@()
  if($Mode -eq 'Configure'){
    foreach($e in $target) {
      $old=Read-Value (Value-Path $e) $e.Name
      if($Reapply -or !$old.Exists -or $old.Kind -ne $e.Kind -or [string]$old.Value -ne [string]$e.Value) {
        if($old.Exists -and $old.Kind -ne 'DWord'){throw 'A VAC setting has an unexpected registry type. No settings changed.'}
        $changes+=$e
        $before+=@{Root=$e.Root;Subkey=$e.Subkey;Name=$e.Name;Exists=$old.Exists;Kind=$old.Kind;Value=$old.Value}
      }
    }
    if(!$changes.Count){
      Verify-Values $target
      if($device.Status -ne 'OK' -or !(Test-StandardPairs)){throw 'Settings match, but standard VAC playback/recording pairs are not verified. Check renamed endpoints manually or rerun with -Reapply.'}
      Write-Host 'VAC settings already match. Four standard playback/recording pairs are present. No writes, elevation or driver restart needed.'
      exit 0
    }
  }
  $admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  if(!$admin) {
    $sourcePath=$PSCommandPath
    if(!$sourcePath){$sourcePath=$env:ANADA_VAC_SCRIPT}
    if(!$sourcePath -or !(Test-Path -LiteralPath $sourcePath)) { throw 'Cannot elevate this invocation. Run the supplied CMD launcher.' }
    $escaped=$sourcePath.Replace("'","''")
    $text="& ([scriptblock]::Create([IO.File]::ReadAllText('$escaped'))) -Mode '$Mode'"
    if($Reapply){$text+=' -Reapply'}
    $log=Join-Path $env:TEMP ('Anada-VAC-'+[guid]::NewGuid().ToString('N')+'.log')
    $escapedLog=$log.Replace("'","''")
    $text="try { Start-Transcript -LiteralPath '$escapedLog' -Force | Out-Null; $text } finally { Stop-Transcript | Out-Null }"
    $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($text))
    $child=Start-Process powershell.exe -ArgumentList @('-NoProfile','-EncodedCommand',$encoded) -Verb RunAs -Wait -PassThru
    if(Test-Path -LiteralPath $log){Get-Content -LiteralPath $log | Write-Host;Write-Host "Setup log: $log"}
    exit $child.ExitCode
  }
  if($Mode -eq 'Restore') {
    $pointer=Join-Path $store 'latest.txt'
    if(!(Test-Path $pointer)){throw 'No VAC setup backup is available.'}
    $folder=(Get-Content -LiteralPath $pointer -Raw).Trim()
    if($folder -notmatch '^backup-[0-9-]+-[a-f0-9]{8}$'){throw 'Invalid backup reference.'}
    $backup=Get-Content -LiteralPath (Join-Path $store "$folder\values.json") -Raw | ConvertFrom-Json
    if($backup.Service -ne $service -or $backup.Product -ne 'VAC4-Anada') {throw 'Backup product does not match.'}
    $entries=@($backup.Values)
    if($entries.Count -lt 1 -or $entries.Count -gt 26){throw 'Invalid backup value count.'}
    $seen=@{}
    foreach($e in $entries) {
      if($e.Root -notin @(0,1) -or $e.Subkey -notmatch '^(|Cable [1-4])$' -or $e.Name -notin @('Number of cables','Minimum number of channels','Maximum number of channels','Volume control') -or ($e.Subkey -eq '' -and $e.Name -ne 'Number of cables') -or ($e.Subkey -ne '' -and $e.Name -eq 'Number of cables') -or ($e.Exists -and $e.Kind -ne 'DWord')){throw 'Backup contains an unsupported registry value.'}
      $identity="$($e.Root)|$($e.Subkey)|$($e.Name)"
      if($seen.ContainsKey($identity)){throw 'Backup contains duplicate values.'};$seen[$identity]=$true
      if($e.Exists -isnot [bool]){throw 'Invalid backup existence flag.'}
      if($e.Exists){
        if($e.Value -isnot [long] -and $e.Value -isnot [int]){throw 'Backup value is not an integer.'}
        $maximum=32;$minimum=0
        if($e.Name -eq 'Number of cables'){$maximum=256}
        if($e.Name -eq 'Volume control'){$minimum=[int]::MinValue;$maximum=[uint32]::MaxValue}
        if($e.Value -lt $minimum -or $e.Value -gt $maximum){throw 'Backup value is outside the supported range.'}
      }
    }
    foreach($e in $entries) {
      $path=Value-Path $e
      if($e.Exists){if(!(Test-Path -LiteralPath $path)){New-Item -Path $path -Force | Out-Null};New-ItemProperty -LiteralPath $path -Name $e.Name -PropertyType DWord -Value $e.Value -Force | Out-Null}
      elseif(Test-Path $path){Remove-ItemProperty -LiteralPath $path -Name $e.Name -ErrorAction SilentlyContinue}
    }
    Verify-Values $entries
    Restart-VAC $device $false
    Write-Host 'Saved VAC values restored. Windows defaults were not changed by this script.'
  } else {
    $folder='backup-'+(Get-Date -Format 'yyyyMMdd-HHmmss')+'-'+[guid]::NewGuid().ToString('N').Substring(0,8)
    $dir=Join-Path $store $folder
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    @{Product='VAC4-Anada';Service=$service;Values=$before}|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $dir 'values.json') -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $store 'latest.txt') -Value $folder -Encoding ASCII
    foreach($e in $changes){$path=Value-Path $e;if(!(Test-Path -LiteralPath $path)){New-Item -Path $path -Force|Out-Null};New-ItemProperty -LiteralPath $path -Name $e.Name -PropertyType DWord -Value $e.Value -Force|Out-Null}
    Verify-Values $target
    Restart-VAC $device
    Write-Host "VAC configured. Backup: $dir"
    Write-Host 'Windows may change defaults during a driver restart. Check your listening output in Windows Sound.'
  }
  exit 0
} catch {
  Write-Host "VAC setup failed: $($_.Exception.Message)" -ForegroundColor Red
  Write-Host 'Close audio applications and try again. Saved backups are in ProgramData\Anada\VACSetup.'
  exit 1
}

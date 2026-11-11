$ErrorActionPreference = 'Stop'
$root = if ($PSScriptRoot) { Split-Path $PSScriptRoot -Parent } else { (Get-Location).Path }
function Inline([string]$value) {
    $value = [Net.WebUtility]::HtmlEncode($value)
    $value = [regex]::Replace($value, '\[([^\]]+)\]\((https://[^)]+)\)', '<a href="$2">$1</a>')
    # The page ships without the repository docs, so relative links become plain text.
    $value = [regex]::Replace($value, '\[([^\]]+)\]\([^)]+\)', '$1')
    $value = [regex]::Replace($value, '\*\*([^*]+)\*\*', '<strong>$1</strong>')
    [regex]::Replace($value, '`([^`]+)`', '<code>$1</code>')
}
$source = Get-Content (Join-Path $root 'docs/GUIDE.md') -Raw -Encoding UTF8
# Join wrapped prose first; numbered actions and table rows remain separate.
$wrapPattern = '(?m)([^\r\n])\r?\n(?!\r?$|#|\d+\. |\|)([^\r\n])'
while ([regex]::IsMatch($source, $wrapPattern)) { $source = [regex]::Replace($source, $wrapPattern, '$1 $2') }
$body = [Collections.Generic.List[string]]::new()
$inList = $false
$inTable = $false
foreach ($line in ($source -split '\r?\n')) {
    if ($line -match '^\d+\. (.*)') {
        if (!$inList) { $body.Add('<ol>'); $inList = $true }
        $body.Add('<li>' + (Inline $Matches[1]) + '</li>')
        continue
    }
    if ($inList) { $body.Add('</ol>'); $inList = $false }
    if ($line.StartsWith('|')) {
        if (!$inTable) { $body.Add('<table>'); $inTable = $true }
        if ($line -match '^\|[\s:|\-]+\|$') { continue }
        $cells = $line.Trim('|').Split('|') | ForEach-Object { '<td>' + (Inline $_.Trim()) + '</td>' }
        $body.Add('<tr>' + ($cells -join '') + '</tr>')
        continue
    }
    if ($inTable) { $body.Add('</table>'); $inTable = $false }
    if ($line -match '^(#{1,3}) (.*)') {
        $level = $Matches[1].Length
        $body.Add("<h$level>" + (Inline $Matches[2]) + "</h$level>")
    } elseif ($line.Trim()) { $body.Add('<p>' + (Inline $line) + '</p>') }
}
if ($inList) { $body.Add('</ol>') }
if ($inTable) { $body.Add('</table>') }
$html = @'
<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Anada user guide</title><style>body{font:17px/1.55 system-ui,sans-serif;color:#20282c;background:#f8faf9;max-width:850px;margin:40px auto;padding:0 24px 60px}h1{font-size:2rem}h2{margin-top:2em;font-size:1.4rem}a{color:#005b62}li{margin:.65em 0}code{background:#e7eeec;padding:2px 5px;overflow-wrap:anywhere}table{border-collapse:collapse;width:100%;margin:1.4em 0}td{border:1px solid #c5d2cf;text-align:left;padding:10px}tr:first-child{font-weight:600;background:#e7eeec}@media(max-width:600px){body{font-size:16px;padding:0 16px;margin:24px auto}h1{font-size:1.7rem}}@media print{body{background:#fff;max-width:none;margin:0}}</style><main>
'@
$html += ($body -join "`n") + '</main></html>'
[IO.File]::WriteAllText((Join-Path $root 'packaging/START-HERE.html'), $html, [Text.UTF8Encoding]::new($false))

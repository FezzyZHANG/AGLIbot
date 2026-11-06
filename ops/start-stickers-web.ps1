# Start the sticker page from the exe named by <runtime>\data\release.json. See docs/DEPLOY.md.
# -Restart first stops the instance recorded in data\logs\stickers-web.json.
param(
    [Parameter(Mandatory)][string]$Runtime,
    [int]$Port = 8790,
    [switch]$Restart
)
$ErrorActionPreference = 'Stop'
$rt = (Resolve-Path -LiteralPath $Runtime).Path
$current = Join-Path $rt 'data\logs\stickers-web.json'

if (Test-Path -LiteralPath $current) {
    $old = Get-Content -Raw -LiteralPath $current | ConvertFrom-Json
    $running = Get-CimInstance Win32_Process -Filter "ProcessId=$($old.pid)" | Where-Object { $_.CommandLine -like '*stickers-web*' }
    if ($running) {
        if (-not $Restart) { "already running pid=$($old.pid) on http://127.0.0.1:$($old.port)"; return }
        Stop-Process -Id $old.pid -Confirm:$false
    }
}

$release = Get-Content -Raw -LiteralPath (Join-Path $rt 'data\release.json') | ConvertFrom-Json
$out = Join-Path $rt 'data\logs\stickers-web.out.log'
$err = Join-Path $rt 'data\logs\stickers-web.err.log'
# WMI starts it outside the caller's process tree, so it survives the shell that ran this.
$line = "cmd.exe /d /s /c `"set `"AGLI_ROOT=$rt`"&& `"$($release.exe)`" stickers-web --port $Port 1>`"$out`" 2>`"$err`"`""
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $line; CurrentDirectory = $rt }
if ($r.ReturnValue -ne 0) { throw "Win32_Process.Create failed: $($r.ReturnValue)" }

$page = $null
for ($i = 0; $i -lt 50 -and -not $page; $i++) {
    Start-Sleep -Milliseconds 200
    $page = Get-CimInstance Win32_Process -Filter "Name='aglibot.exe' AND ParentProcessId=$($r.ProcessId)"
}
if (-not $page) { throw "sticker page did not start; see $err" }
$record = [ordered]@{ pid = $page.ProcessId; port = $Port; exe = $release.exe; started = (Get-Date -Format o) }
[IO.File]::WriteAllText($current, ($record | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
"stickers at http://127.0.0.1:$Port pid=$($page.ProcessId)"

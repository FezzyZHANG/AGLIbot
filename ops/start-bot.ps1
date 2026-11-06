# Start the bot named by <runtime>\data\release.json. See docs/DEPLOY.md.
# -Restart first stops the instance recorded in data\logs\current.json.
param(
    [Parameter(Mandatory)][string]$Runtime,
    [switch]$Restart
)
$ErrorActionPreference = 'Stop'
$rt = (Resolve-Path -LiteralPath $Runtime).Path
$cfg = Get-Content -Raw -LiteralPath (Join-Path $rt 'runtime.json') | ConvertFrom-Json
$current = Join-Path $rt 'data\logs\current.json'

if ($Restart -and (Test-Path -LiteralPath $current)) {
    $old = Get-Content -Raw -LiteralPath $current | ConvertFrom-Json
    $proc = Get-Process -Id $old.pid -ErrorAction SilentlyContinue
    if ($proc -and $proc.Name -eq 'aglibot') {
        if ($proc.Path -ne $old.exe) { throw "pid $($old.pid) runs $($proc.Path), not the recorded $($old.exe); stop it explicitly" }
        $window = (Get-CimInstance Win32_Process -Filter "ProcessId=$($old.pid)").ParentProcessId
        Stop-Process -Id $old.pid -Confirm:$false
        $proc.WaitForExit(30000) | Out-Null
        # Its cmd.exe is now paused on the exit message; close that window too.
        if ($window -and (Get-CimInstance Win32_Process -Filter "Name='cmd.exe' AND ProcessId=$window")) {
            Stop-Process -Id $window -Confirm:$false -ErrorAction SilentlyContinue
        }
    }
}
# The sticker page runs from the same exe, so tell the two apart by command line.
if (Get-CimInstance Win32_Process -Filter "Name='aglibot.exe'" | Where-Object { $_.CommandLine -notlike '*stickers-web*' }) { throw 'AGLI is already running; stop the intended instance explicitly before another start.' }

$release = Get-Content -Raw -LiteralPath (Join-Path $rt 'data\release.json') | ConvertFrom-Json
$sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $release.exe).Hash.ToLower()
if ($sha -ne $release.exe_sha256) { throw "release executable changed since it was recorded: $($release.exe)" }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = Join-Path $rt "data\logs\bot-$stamp.out.log"
$err = Join-Path $rt "data\logs\bot-$stamp.err.log"
# WMI starts the bot outside the caller's process tree, so it survives the shell (and Claude Code) that ran this.
# The bot prints its log to this window and copies it to AGLI_LOG_FILE. Stderr (panics, startup errors)
# goes to its file and is shown once the bot exits; the pause keeps the window open to read it.
$line = "cmd.exe /d /s /c `"set `"AGLI_ROOT=$rt`"&& set `"AGLI_SOURCE=$($cfg.source)`"&& set `"AGLI_LOG_FILE=$out`"&& set NO_COLOR=1&& `"$($release.exe)`" 2>`"$err`" & chcp 65001 >nul & type `"$err`" & echo bot exited, log: `"$out`" & pause`""
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $line; CurrentDirectory = $rt }
if ($r.ReturnValue -ne 0) { throw "Win32_Process.Create failed: $($r.ReturnValue)" }

$bot = $null
for ($i = 0; $i -lt 50 -and -not $bot; $i++) {
    Start-Sleep -Milliseconds 200
    $bot = Get-CimInstance Win32_Process -Filter "Name='aglibot.exe' AND ParentProcessId=$($r.ProcessId)"
}
if (-not $bot) { throw "bot did not start; see $err" }
$record = [ordered]@{ pid = $bot.ProcessId; exe = $release.exe; commit = $release.commit; origin = $release.origin
    root = $rt; out = $out; err = $err; started = (Get-Date -Format o) }
[IO.File]::WriteAllText($current, ($record | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
"started $($release.commit) ($($release.origin)) pid=$($bot.ProcessId); log $out"

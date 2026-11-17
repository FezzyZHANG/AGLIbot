# Audit fixture: run the real start script with synthetic files and mocked process APIs.
# No process is started or stopped; all process and hash operations below are functions.
$ErrorActionPreference = 'Stop'
$sourceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$auditFixture = Join-Path $sourceRoot 'target/audit-20261115/ops-mock-runtime'
New-Item -ItemType Directory -Force -Path (Join-Path $auditFixture 'data/logs') | Out-Null
[IO.File]::WriteAllText((Join-Path $auditFixture 'runtime.json'), '{"source":"synthetic-source"}')
[IO.File]::WriteAllText((Join-Path $auditFixture 'data/logs/current.json'), '{"pid":111,"exe":"old.exe"}')
[IO.File]::WriteAllText((Join-Path $auditFixture 'data/release.json'), '{"commit":"next","exe":"missing.exe","exe_sha256":"bad"}')
$global:auditMockProcesses = @{}
$global:auditMockStopped = [Collections.Generic.List[int]]::new()
function Get-Process {
    param([int]$Id, $ErrorAction)
    $global:auditMockProcesses[$Id]
}
function Stop-Process {
    param([int]$Id, [switch]$Confirm, $ErrorAction)
    $global:auditMockStopped.Add($Id)
    $global:auditMockProcesses.Remove($Id)
}
function Get-CimInstance {
    param([string]$ClassName, [string]$Filter)
    if ($Filter -match '^ProcessId=(\d+)$') {
        if ($global:auditMockProcesses.ContainsKey([int]$Matches[1])) {
            [pscustomobject]@{ParentProcessId=999}
        }
    } elseif ($Filter -eq "Name='aglibot.exe'") {
        foreach ($item in $global:auditMockProcesses.Values) {
            [pscustomobject]@{ProcessId=$item.Id; CommandLine='synthetic-bot'}
        }
    } elseif ($Filter -like "Name='cmd.exe'*") {
        return
    } else {
        throw "Unexpected process query in audit fixture: $Filter"
    }
}
function Get-FileHash {
    param($Algorithm, $LiteralPath)
    throw 'synthetic release hash failure'
}
function Invoke-CimMethod { throw 'Unexpected attempt to start a process in audit fixture' }
$startScript = Join-Path $sourceRoot 'ops/start-bot.ps1'

# Case 1: a damaged next release must not stop a healthy current process.
$healthy = [pscustomobject]@{Id=111;Name='aglibot';Path='old.exe'}
$healthy | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value { param($milliseconds) $true }
$global:auditMockProcesses[111] = $healthy
try { & $startScript -Runtime $auditFixture -Restart; throw 'Expected failure was absent' }
catch {
    if ($_.Exception.Message -ne 'synthetic release hash failure') { throw }
    if (-not $global:auditMockStopped.Contains(111)) { throw 'Ordering observation no longer reproduced' }
    'CONFIRMED: -Restart stops the healthy recorded process before a candidate hash failure.'
}

# Case 2: evolution replaces the process but the recorded PID still refers to its predecessor.
$global:auditMockProcesses = @{222=[pscustomobject]@{Id=222;Name='aglibot';Path='evolved.exe'}}
$global:auditMockStopped.Clear()
try { & $startScript -Runtime $auditFixture -Restart; throw 'Expected failure was absent' }
catch {
    if ($_.Exception.Message -notlike 'AGLI is already running*') { throw }
    if ($global:auditMockStopped.Count -ne 0 -or -not $global:auditMockProcesses.ContainsKey(222)) {
        throw 'Stale PID observation no longer reproduced'
    }
    'CONFIRMED: stale current.json leaves the evolved process running and -Restart refuses it.'
}

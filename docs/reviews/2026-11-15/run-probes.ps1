# Exercise production Rust modules unchanged, using synthetic data and model stubs.
$ErrorActionPreference = 'Stop'
$sourceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$probeRoot = Join-Path $sourceRoot 'target/audit-20261115/probes'
$tempRoot = Join-Path $sourceRoot 'target/audit-20261115/tmp'
New-Item -ItemType Directory -Path $probeRoot, $tempRoot -Force | Out-Null
$testFile = (Join-Path $PSScriptRoot 'audit_probes.rs') | ConvertTo-Json -Compress
$manifest = [IO.File]::ReadAllText((Join-Path $sourceRoot 'Cargo.toml'))
$manifest += "`n[[bin]]`nname = 'aglibot_audit_probes'`npath = $testFile`n"
[IO.File]::WriteAllText((Join-Path $probeRoot 'Cargo.toml'), $manifest, [Text.UTF8Encoding]::new($false))
Copy-Item -LiteralPath (Join-Path $sourceRoot 'Cargo.lock') -Destination (Join-Path $probeRoot 'Cargo.lock')
$priorTemp = $env:TEMP
$priorTmp = $env:TMP
try {
    $env:TEMP = $tempRoot
    $env:TMP = $tempRoot
    & cargo test --manifest-path (Join-Path $probeRoot 'Cargo.toml') --target-dir (Join-Path $sourceRoot 'target') --locked --offline audit_probes:: -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Audit probes failed with exit code $LASTEXITCODE" }
} finally {
    $env:TEMP = $priorTemp
    $env:TMP = $priorTmp
}

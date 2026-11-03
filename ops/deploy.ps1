# Build a commit as a release, record it in <runtime>\data\release.json and restart the bot.
# Refuses to replace the current release with a commit that does not contain it (for example
# an evolution on an unmerged codex/evolution-* branch) unless -Force. See docs/DEPLOY.md.
param(
    [Parameter(Mandatory)][string]$Runtime,
    [string]$Commit = 'main',
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
$rt = (Resolve-Path -LiteralPath $Runtime).Path
$cfg = Get-Content -Raw -LiteralPath (Join-Path $rt 'runtime.json') | ConvertFrom-Json
$src = $cfg.source
$wt = $cfg.deploy_worktree

$target = (git -C $src rev-parse --verify "$Commit^{commit}")
if ($LASTEXITCODE -ne 0) { throw "unknown commit $Commit" }
$pointer = Join-Path $rt 'data\release.json'
if (Test-Path -LiteralPath $pointer) {
    $cur = Get-Content -Raw -LiteralPath $pointer | ConvertFrom-Json
    if ($cur.commit -ne $target) {
        git -C $src merge-base --is-ancestor $cur.commit $target
        if ($LASTEXITCODE -ne 0 -and -not $Force) {
            throw "$Commit ($target) does not contain the running release $($cur.commit) ($($cur.origin)); merge it first or pass -Force"
        }
    }
}

if (git -C $wt status --porcelain) { throw "deploy worktree $wt has local changes" }
git -C $wt checkout -q --detach $target
if ($LASTEXITCODE -ne 0) { throw "checkout failed in $wt" }
$targetDir = Join-Path $rt "data\releases\$target"
Push-Location $wt
try {
    cargo build --locked --release --bin aglibot --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }
} finally { Pop-Location }
$exe = Join-Path $targetDir 'release\aglibot.exe'

# Runtime config is deliberately its own copy; show where it differs from this commit's defaults.
"config differences (commit vs runtime):"
git --no-pager diff --no-index --stat -- (Join-Path $wt 'config') (Join-Path $rt 'config')
if ($LASTEXITCODE -eq 0) { "  none" }

$release = [ordered]@{ commit = $target; exe = $exe; exe_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash.ToLower()
    origin = 'manual'; at = (Get-Date).ToUniversalTime().ToString('o') }
[IO.File]::WriteAllText($pointer, ($release | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
& (Join-Path $PSScriptRoot 'start-bot.ps1') -Runtime $rt -Restart

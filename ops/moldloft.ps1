# Run the Moldloft v2 CLI against this project's workspace. See README.md (开发管理).
# No arguments: serve the workbench on the LAN, http://127.0.0.1:8787 locally.
# Otherwise arguments pass through: `ops/moldloft.ps1 ready --json`, `ops/moldloft.ps1 mcp`,
# `ops/moldloft.ps1 export workspace/moldloft/archive`.
# After a Moldloft dependency change: .tools\moldloft\Scripts\python.exe -m pip install -e Moldloft
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$venv = Join-Path $root '.tools\moldloft'
$exe = Join-Path $venv 'Scripts\moldloft.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    if (-not (Test-Path -LiteralPath (Join-Path $root 'Moldloft\pyproject.toml'))) {
        throw 'Moldloft/ is empty; run: git submodule update --init Moldloft'
    }
    python -m venv $venv
    if ($LASTEXITCODE) { throw 'python -m venv failed' }
    & (Join-Path $venv 'Scripts\python.exe') -m pip install -q -e (Join-Path $root 'Moldloft')
    if ($LASTEXITCODE) { throw 'installing Moldloft failed' }
}
$cli = [string[]]$(if ($args.Count) { $args } else { 'serve', '--listen' })
$db = Join-Path $root 'workspace\moldloft\data\moldloft.db'
& $exe --db $db --repo "aglibot=$root" @cli
exit $LASTEXITCODE

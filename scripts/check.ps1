$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
# Windows PowerShell 5.1 treats native stderr as errors when redirected.
# Cargo writes progress to stderr even on success; use its exit code.
function Invoke-CargoChecked {
    param([string[]]$CargoArguments)
    $ErrorActionPreference = 'Continue'
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed: $($CargoArguments -join ' ')" }
}
Push-Location $projectRoot
try {
    Invoke-CargoChecked @('fmt', '--all', '--', '--check')
    # Win32 IPC tests share OS message timing; serialize them for reproducible checks.
    Invoke-CargoChecked @('test', '--locked', '--lib', '--no-default-features', '--', '--test-threads=1')
    Invoke-CargoChecked @('check', '--locked', '--all-targets')
} finally { Pop-Location }

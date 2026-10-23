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
    Invoke-CargoChecked @('fmt', '--manifest-path', 'crates/i18n-macros/Cargo.toml', '--', '--check')
    # Win32 IPC tests share OS message timing; serialize them for reproducible checks.
    Invoke-CargoChecked @('test', '--locked', '--lib', '--no-default-features', '--', '--test-threads=1')
    # The Shell-link test initializes a COM apartment, so it runs in its own process;
    # sharing the unit-test thread tore the apartment down and crashed the next test.
    Invoke-CargoChecked @('test', '--locked', '--test', 'shortcut_targets')
    Invoke-CargoChecked @('check', '--locked', '--all-targets')
} finally { Pop-Location }

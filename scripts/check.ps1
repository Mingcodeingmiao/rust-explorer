param([switch]$SkipInteractiveShellTests)
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
    # Compile the interactive OLE test everywhere, with its ignored reason visible.
    Invoke-CargoChecked @('test', '--locked', '--test', 'native_drag', '--', '--test-threads=1')
    if (-not $SkipInteractiveShellTests) {
        # A Shell Drop can block without an interactive desktop. The test runs
        # in a bounded child process and uses only disposable local fixtures.
        Invoke-CargoChecked @('test', '--locked', '--test', 'native_drag', '--', '--ignored', '--test-threads=1')
    } else {
        Write-Host 'Interactive Shell drag/drop execution omitted; run scripts/check.ps1 on an unlocked Windows desktop before release.'
    }
    Invoke-CargoChecked @('check', '--locked', '--all-targets')
} finally { Pop-Location }

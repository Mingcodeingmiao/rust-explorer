$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$output = Join-Path $root 'artifacts/shell-bridge-experiment'
New-Item -ItemType Directory -Path $output -Force | Out-Null
$compiler = Get-Command g++ -ErrorAction Stop
$arguments = @('-std=c++17', '-O2', '-DUNICODE', '-D_UNICODE', '-municode', '-Wno-attributes')
$libraries = @('-lole32', '-loleaut32', '-lshell32', '-luuid', '-lpropsys')
foreach ($program in @('shell-bridge', 'probe')) {
    $sources = @(if ($program -eq 'shell-bridge') {
        @((Join-Path $PSScriptRoot 'main.cpp'), (Join-Path $PSScriptRoot 'OpenInFolder.cpp'))
    } else { @((Join-Path $PSScriptRoot 'probe.cpp')) })
    $ErrorActionPreference = 'Continue' # PowerShell 5 treats native stderr notes as errors.
    & $compiler.Source @arguments @sources '-o' (Join-Path $output ($program + '.exe')) @libraries
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($exitCode -ne 0) { throw "Build failed: $program" }
}

param([switch]$Release, [switch]$Gnu)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargoPath)) { throw '找不到 Rust，请先安装 rustup。' }
$env:PATH = (Split-Path -Parent $cargoPath) + ';' + $env:PATH
$buildArgs = @()
if ($Gnu) {
    if (-not (Test-Path 'C:\msys64\ucrt64\bin\gcc.exe')) { throw 'GNU 构建需要 MSYS2 UCRT64 GCC。' }
    $env:PATH = 'C:\msys64\ucrt64\bin;' + $env:PATH
    $buildArgs += '+stable-x86_64-pc-windows-gnu'
}
$buildArgs += @('build', '--locked')
if ($Release) {
    $buildArgs += '--release'
    $sdkBins = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $shaderCompiler = Get-ChildItem -LiteralPath $sdkBins -Filter fxc.exe -Recurse -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
    if ($shaderCompiler) { $env:GPUI_FXC_PATH = $shaderCompiler.FullName }
}
Push-Location $projectRoot
try {
    $previousAction = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $cargoPath @buildArgs } finally { $ErrorActionPreference = $previousAction }
    if ($LASTEXITCODE -ne 0) { throw 'Cargo 构建失败。' }
    $profile = if ($Release) { 'release' } else { 'debug' }
    $outputDir = Join-Path $projectRoot 'dist\RustExplorer-windows-x64-native'
    New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\$profile\rust-explorer.exe") -Destination $outputDir -Force
    $sdkDll = Join-Path $projectRoot '.tools\everything-sdk\dll\Everything64.dll'
    if (Test-Path -LiteralPath $sdkDll) {
        Copy-Item -LiteralPath $sdkDll -Destination $outputDir -Force
        Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\Everything-SDK-License.txt') -Destination $outputDir -Force
    }
    if ($Gnu) {
        foreach ($dllName in @('libgcc_s_seh-1.dll', 'libwinpthread-1.dll')) {
            $runtimeDll = Join-Path $env:USERPROFILE ".rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\$dllName"
            if (Test-Path -LiteralPath $runtimeDll) { Copy-Item -LiteralPath $runtimeDll -Destination $outputDir -Force }
        }
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination $outputDir -Force
    $docsDir = Join-Path $outputDir 'docs'
    New-Item -ItemType Directory -Force -Path $docsDir | Out-Null
    Copy-Item -Path (Join-Path $projectRoot 'docs\*.md') -Destination $docsDir -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination $outputDir -Force
    foreach ($notice in @('NOTICE', 'THIRD_PARTY_NOTICES.md', 'README.en.md', 'KNOWN_ISSUES.md', 'CHANGELOG.md', 'resources\pinyin-LICENSE.txt')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot $notice) -Destination $outputDir -Force
    }
    # Keep the previously published entry points on the same native build.
    $legacyOutput = Join-Path $projectRoot 'dist\RustExplorer-windows-x64'
    New-Item -ItemType Directory -Force -Path $legacyOutput | Out-Null
    Copy-Item -Path (Join-Path $outputDir '*') -Destination $legacyOutput -Recurse -Force
    $legacyWebView = Join-Path $legacyOutput 'WebView2Loader.dll'
    if (Test-Path -LiteralPath $legacyWebView) { Remove-Item -LiteralPath $legacyWebView -Force }
    $rootWebView = Join-Path $projectRoot 'dist\WebView2Loader.dll'
    if (Test-Path -LiteralPath $rootWebView) { Remove-Item -LiteralPath $rootWebView -Force }
    foreach ($name in @('rust-explorer.exe', 'Everything64.dll', 'libgcc_s_seh-1.dll', 'libwinpthread-1.dll')) {
        $nativeFile = Join-Path $outputDir $name
        if (Test-Path -LiteralPath $nativeFile) {
            Copy-Item -LiteralPath $nativeFile -Destination (Join-Path $projectRoot "dist\$name") -Force
        }
    }
    Write-Host "构建完成：$outputDir\rust-explorer.exe"
} finally { Pop-Location }

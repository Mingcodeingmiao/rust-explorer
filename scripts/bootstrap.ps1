param([switch]$SkipEverythingSdk)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$toolsDir = Join-Path $projectRoot '.tools'
New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
$cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargoPath)) {
    throw '请先安装 Rust：https://rustup.rs/。推荐 MSVC 工具链及 Visual Studio C++ Build Tools。'
}
if (-not $SkipEverythingSdk) {
    $sdkDir = Join-Path $toolsDir 'everything-sdk'
    if (-not (Test-Path -LiteralPath (Join-Path $sdkDir 'dll\Everything64.dll'))) {
        $archivePath = Join-Path $toolsDir 'Everything-SDK.zip'
        Invoke-WebRequest 'https://www.voidtools.com/Everything-SDK.zip' -OutFile $archivePath
        Expand-Archive -LiteralPath $archivePath -DestinationPath $sdkDir -Force
    }
}
Write-Host '依赖准备完成。执行 scripts\build.ps1 构建。'

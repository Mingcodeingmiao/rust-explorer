param([string]$BuildDirectory = 'dist\RustExplorer-windows-x64-native')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw
if ($manifest -notmatch '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"') { throw 'Cannot determine package version.' }
$version = $Matches[1]
$source = if ([IO.Path]::IsPathRooted($BuildDirectory)) { $BuildDirectory } else { Join-Path $projectRoot $BuildDirectory }
if (-not (Test-Path -LiteralPath (Join-Path $source 'rust-explorer.exe'))) { throw 'Build the application first.' }
$outputRoot = Join-Path $projectRoot 'artifacts'
New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
$staging = Join-Path $outputRoot ("package-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
foreach ($name in @('rust-explorer.exe', 'Everything64.dll', 'libgcc_s_seh-1.dll', 'libwinpthread-1.dll')) {
    $file = Join-Path $source $name
    if (Test-Path -LiteralPath $file) { Copy-Item -LiteralPath $file -Destination $staging }
}
foreach ($name in @('README.md', 'README.en.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'CHANGELOG.md', 'KNOWN_ISSUES.md')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot $name) -Destination $staging
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\pinyin-LICENSE.txt') -Destination $staging
Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\files-MIT.txt') -Destination $staging
Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\fluent-icons\LICENSE') -Destination (Join-Path $staging 'FluentUI-System-Icons-LICENSE.txt')
if (Test-Path -LiteralPath (Join-Path $staging 'Everything64.dll')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\Everything-SDK-License.txt') -Destination $staging
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'docs') -Destination $staging -Recurse
New-Item -ItemType Directory -Path (Join-Path $staging 'resources') | Out-Null
Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\app-icon') -Destination (Join-Path $staging 'resources') -Recurse
Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\pinyin-LICENSE.txt') -Destination (Join-Path $staging 'resources')
Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\Everything-SDK-License.txt') -Destination (Join-Path $staging 'resources')
if ((Test-Path -LiteralPath (Join-Path $staging 'libgcc_s_seh-1.dll')) -or (Test-Path -LiteralPath (Join-Path $staging 'libwinpthread-1.dll'))) {
    Copy-Item -LiteralPath (Join-Path $projectRoot 'resources\licenses') -Destination $staging -Recurse
}
$archive = Join-Path $outputRoot "RustExplorer-$version-windows-x64.zip"
Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $archive -Force
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $outputRoot 'SHA256SUMS.txt'), "$hash  $([IO.Path]::GetFileName($archive))`n", [Text.UTF8Encoding]::new($false))
Write-Host "Package: $archive"
Write-Host "SHA256: $hash"
Write-Host "Staging retained for inspection: $staging"

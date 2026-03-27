param(
    [string]$Configuration = "release"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$uiRoot = Join-Path $repoRoot "saba\ui\cosmo-browse-ui"
$sabaRoot = Join-Path $repoRoot "saba"
$packageJsonPath = Join-Path $uiRoot "package.json"
$tauriConfigPath = Join-Path $uiRoot "src-tauri\tauri.conf.json"

$packageJson = Get-Content $packageJsonPath -Raw | ConvertFrom-Json
$tauriConfig = Get-Content $tauriConfigPath -Raw | ConvertFrom-Json
$version = $packageJson.version
$productName = $tauriConfig.productName

$artifactRoot = Join-Path $repoRoot ("artifacts\{0}" -f $productName)
$portableRoot = Join-Path $artifactRoot "windows-portable"
$versionRoot = Join-Path $portableRoot $version
$zipPath = Join-Path $artifactRoot ("{0}-{1}-windows-portable.zip" -f $productName, $version)
$exeSourcePath = Join-Path $sabaRoot ("target\{0}\{1}.exe" -f $Configuration, $productName)
$exeOutputPath = Join-Path $versionRoot ("{0}.exe" -f $productName)
$readmePath = Join-Path $versionRoot "README.txt"
$buildInfoPath = Join-Path $versionRoot "BUILD-INFO.txt"
$shaPath = Join-Path $versionRoot "SHA256SUMS.txt"
$gitCommit = (git -C $repoRoot rev-parse --short HEAD).Trim()
$buildTimestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

New-Item -ItemType Directory -Force $artifactRoot | Out-Null
New-Item -ItemType Directory -Force $portableRoot | Out-Null
New-Item -ItemType Directory -Force $versionRoot | Out-Null

Push-Location $uiRoot
try {
    npm run build
    if ($LASTEXITCODE -ne 0) {
        throw "npm run build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

Push-Location $sabaRoot
try {
    cargo build -p cosmo-browse-ui --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

if (-not (Test-Path $exeSourcePath)) {
    throw "Built executable was not found at $exeSourcePath"
}

Copy-Item $exeSourcePath $exeOutputPath -Force

$readme = @"
CosmoBrowse portable trial package

1. Run $productName.exe
2. Open https://abehiroshi.la.coocan.jp/
3. Verify left menu navigation and one external _blank link

If the app does not start, install the Microsoft Edge WebView2 Evergreen Runtime and try again.
"@
Set-Content $readmePath $readme

$buildInfo = @"
Product: $productName
Version: $version
Commit: $gitCommit
BuiltAtUtc: $buildTimestamp
Configuration: $Configuration
"@
Set-Content $buildInfoPath $buildInfo

$shaLines = Get-ChildItem $versionRoot -File |
    Sort-Object Name |
    ForEach-Object {
        $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "{0}  {1}" -f $hash, $_.Name
    }
Set-Content $shaPath $shaLines

if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}
Compress-Archive -Path (Join-Path $versionRoot "*") -DestinationPath $zipPath

Write-Host "Created portable directory: $versionRoot"
Write-Host "Created zip archive: $zipPath"

param(
    [string]$OutputDir = "dist",
    [switch]$SkipTests,
    [switch]$NoArchive,
    [switch]$AllowDirty
)

$ErrorActionPreference = "Stop"

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $Root

if (-not $AllowDirty) {
    $Dirty = git status --porcelain
    if ($Dirty) {
        throw "Working tree is not clean. Commit or stash changes, or rerun with -AllowDirty for a local-only package."
    }
}

$HeadSha = (git rev-parse HEAD).Trim()
$ShortSha = (git rev-parse --short=12 HEAD).Trim()
$CargoMetadata = cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
$Version = ($CargoMetadata.packages | Where-Object { $_.name -eq "opolys-core" } | Select-Object -First 1).version
$RustcVersion = (rustc --version).Trim()
$CargoVersion = (cargo --version).Trim()
$HostTriple = ((rustc -vV) | Where-Object { $_ -like "host:*" }).Replace("host: ", "").Trim()
$BinaryExt = if ($IsWindows -or $env:OS -eq "Windows_NT") { ".exe" } else { "" }
$PackageName = "opolys-$Version-$ShortSha-$HostTriple"
$OutputRoot = Join-Path $Root $OutputDir
$StageDir = Join-Path $OutputRoot $PackageName

if (Test-Path $StageDir) {
    Remove-Item -Recurse -Force $StageDir
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

if (-not $SkipTests) {
    cargo test --locked -p opolys-consensus
    cargo test --locked -p opolys-node
    cargo test --locked --manifest-path vendor/evo-omap/Cargo.toml
}

cargo build --locked --release -p opolys-node -p opolys-wallet -p genesis-ceremony -p opolys-crypto

$Binaries = @(
    "opolys-node",
    "opl",
    "genesis-ceremony",
    "genesis-keys"
)

foreach ($Binary in $Binaries) {
    $Source = Join-Path $Root "target\release\$Binary$BinaryExt"
    if (-not (Test-Path $Source)) {
        throw "Expected release binary not found: $Source"
    }
    Copy-Item $Source (Join-Path $StageDir "$Binary$BinaryExt")
}

Copy-Item (Join-Path $Root "README.md") (Join-Path $StageDir "README.md")
Copy-Item (Join-Path $Root "docs\MAINNET_LAUNCH.md") (Join-Path $StageDir "MAINNET_LAUNCH.md")
Copy-Item (Join-Path $Root "docs\OPERATOR_CONFIG.md") (Join-Path $StageDir "OPERATOR_CONFIG.md")
Copy-Item (Join-Path $Root "docs\THREAT_MODEL.md") (Join-Path $StageDir "THREAT_MODEL.md")
Copy-Item (Join-Path $Root "docs\RELEASE.md") (Join-Path $StageDir "RELEASE.md")

$Manifest = [ordered]@{
    package = $PackageName
    version = $Version
    git_commit = $HeadSha
    host_triple = $HostTriple
    rustc = $RustcVersion
    cargo = $CargoVersion
    cargo_locked = $true
    generated_utc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    binaries = $Binaries | ForEach-Object { "$_$BinaryExt" }
}
$Manifest | ConvertTo-Json -Depth 8 | Out-File (Join-Path $StageDir "release-manifest.json") -Encoding utf8

$ChecksumFiles = Get-ChildItem $StageDir -File | Sort-Object Name
$ChecksumLines = foreach ($File in $ChecksumFiles) {
    $Hash = (Get-FileHash -Algorithm SHA256 $File.FullName).Hash.ToLowerInvariant()
    "$Hash  $($File.Name)"
}
$ChecksumLines | Out-File (Join-Path $StageDir "SHA256SUMS.txt") -Encoding ascii

if (-not $NoArchive) {
    $ArchivePath = Join-Path $OutputRoot "$PackageName.zip"
    if (Test-Path $ArchivePath) {
        Remove-Item -Force $ArchivePath
    }
    Compress-Archive -Path (Join-Path $StageDir "*") -DestinationPath $ArchivePath
    $ArchiveHash = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToLowerInvariant()
    "$ArchiveHash  $(Split-Path -Leaf $ArchivePath)" | Out-File (Join-Path $OutputRoot "$PackageName.zip.sha256") -Encoding ascii
}

Write-Host "Release package written to $StageDir"
Write-Host "Checksums written to $(Join-Path $StageDir 'SHA256SUMS.txt')"
if (-not $NoArchive) {
    Write-Host "Archive written to $ArchivePath"
}

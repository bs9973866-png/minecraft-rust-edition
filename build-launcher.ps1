$ErrorActionPreference = "Stop"

Write-Host "Building Rust Edition game and launcher..." -ForegroundColor Cyan
cargo build --release -p rust-edition-client -p rust-edition-launcher
if ($LASTEXITCODE -ne 0) {
    throw "Cargo build failed."
}

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$source = Join-Path $root "target\release"
$dist = Join-Path $root "dist\RustEdition"
New-Item -ItemType Directory -Force -Path $dist | Out-Null

$files = @("rust-edition-launcher.exe", "rust-edition.exe")
foreach ($file in $files) {
    $from = Join-Path $source $file
    if (-not (Test-Path $from)) {
        throw "Build output not found: $from"
    }
    Copy-Item -Force $from $dist
}

Write-Host ""
Write-Host "Portable folder created: $dist" -ForegroundColor Green
Write-Host "Run rust-edition-launcher.exe. Keep both EXE files together."

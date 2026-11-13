#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    Builds standalone release binaries of mod-fixer for Windows x64, Linux x64, and Linux ARM64.
#>
param(
    [switch]$SkipTests
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RootDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Manifest = Join-Path $RootDir 'Cargo.toml'
$DistDir = Join-Path $RootDir 'dist'

if ($IsLinux) {
    $sep = [System.IO.Path]::PathSeparator
    $env:PATH = "/usr/local/bin$sep$HOME/.cargo/bin$sep$env:PATH"
}

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} elseif ($IsLinux -and (Test-Path '/var/tmp/mod4win-target')) {
    '/var/tmp/mod4win-target'
} else {
    Join-Path $RootDir 'target'
}
$env:CARGO_TARGET_DIR = $TargetDir

if (-not $SkipTests) {
    Write-Host '[Test] Running unit & integration tests + clippy...' -ForegroundColor Cyan
    & cargo test --manifest-path $Manifest
    if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
    & cargo clippy --manifest-path $Manifest --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
}

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

if ($IsWindows) {
    Write-Host '[Build] Windows x86_64 (native)...' -ForegroundColor Yellow
    & cargo build --release --manifest-path $Manifest --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Windows x64 build failed' }
} else {
    Write-Host '[Build] Windows x86_64 (cargo xwin)...' -ForegroundColor Yellow
    & cargo xwin build --release --manifest-path $Manifest --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Windows x64 cross-build failed' }

    Write-Host '[Build] Linux x86_64 (glibc 2.28 via cargo zigbuild)...' -ForegroundColor Yellow
    & cargo zigbuild --release --manifest-path $Manifest --target x86_64-unknown-linux-gnu.2.28
    if ($LASTEXITCODE -ne 0) { throw 'Linux x64 build failed' }

    Write-Host '[Build] Linux ARM64 (glibc 2.28 via cargo zigbuild)...' -ForegroundColor Yellow
    & cargo zigbuild --release --manifest-path $Manifest --target aarch64-unknown-linux-gnu.2.28
    if ($LASTEXITCODE -ne 0) { throw 'Linux ARM64 build failed' }
}

$Artifacts = @(
    @{ Src = Join-Path $TargetDir 'x86_64-pc-windows-msvc/release/mod-fixer.exe'; Dst = 'mod-fixer-win-x64.exe' },
    @{ Src = Join-Path $TargetDir 'x86_64-unknown-linux-gnu/release/mod-fixer';   Dst = 'mod-fixer-linux-x64' },
    @{ Src = Join-Path $TargetDir 'aarch64-unknown-linux-gnu/release/mod-fixer';  Dst = 'mod-fixer-linux-arm64' }
)

$Checksums = @()
foreach ($item in $Artifacts) {
    if (Test-Path $item.Src) {
        $outFile = Join-Path $DistDir $item.Dst
        Copy-Item -Force $item.Src $outFile
        if ($IsLinux -and -not $item.Dst.EndsWith('.exe')) {
            & chmod +x $outFile
        }
        $hash = (Get-FileHash -Algorithm SHA256 $outFile).Hash.ToLowerInvariant()
        $sizeKiB = [Math]::Round((Get-Item $outFile).Length / 1024.0, 1)
        Write-Host ("  -> {0,-26} ({1,6} KiB)  sha256:{2}" -f $item.Dst, $sizeKiB, $hash) -ForegroundColor Green
        $Checksums += "$hash  $($item.Dst)"
    }
}

$SumPath = Join-Path $DistDir 'SHA256SUMS.txt'
[System.IO.File]::WriteAllLines($SumPath, $Checksums)
Write-Host "[Done] Release artifacts written to $DistDir" -ForegroundColor Cyan

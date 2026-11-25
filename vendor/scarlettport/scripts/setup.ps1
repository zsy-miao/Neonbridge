# Builds the runtime, fetches the D3D12 Agility SDK (needed on Windows 10) and, with -Game,
# precompiles every shader of the game so it never waits on the shader compiler mid-game.
# Usage: .\scripts\setup.ps1 [-Game "D:\path\to\dump"]
param([string]$Game)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$bin  = Join-Path $root 'runtime\xcl\target\release'

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust is not installed. Get it from https://rustup.rs (stable-x86_64-pc-windows-msvc).'
}

Write-Host '==> Building XCL runtime (release)...' -ForegroundColor Cyan
Push-Location (Join-Path $root 'runtime\xcl')
cargo build --release
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { throw "cargo build failed (exit $code)." }

$agility = Join-Path $bin 'D3D12'
if (-not (Test-Path (Join-Path $agility 'D3D12Core.dll'))) {
    Write-Host '==> Downloading D3D12 Agility SDK 1.614.1...' -ForegroundColor Cyan
    $tmp = Join-Path $env:TEMP 'agility-sdk'
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $pkg = Join-Path $tmp 'agility.zip'
    Invoke-WebRequest 'https://www.nuget.org/api/v2/package/Microsoft.Direct3D.D3D12/1.614.1' -OutFile $pkg
    Expand-Archive $pkg -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force $agility | Out-Null
    Copy-Item (Join-Path $tmp 'build\native\bin\x64\D3D12Core.dll') $agility
    Copy-Item (Join-Path $tmp 'build\native\bin\x64\d3d12SDKLayers.dll') $agility
}

if ($Game) {
    $pack = Join-Path $Game 'shaders.rpf'
    if (-not (Test-Path $pack)) { throw "No shaders.rpf in '$Game'." }
    Write-Host '==> Precompiling all ~26,500 shaders + GPU pipelines (about 45 min, once; resumable)...' -ForegroundColor Cyan
    $env:XCL_DXC_THREADS = '8'; $env:XCL_PIPELINE_THREADS = '3'
    & (Join-Path $bin 'xcl.exe') shaders $pack --precompile --pipelines
    if ($LASTEXITCODE -ne 0) { throw "Shader precompile failed (exit $LASTEXITCODE). Run setup.ps1 again to resume." }
}

Write-Host ''
Write-Host 'Done. Run the game with:' -ForegroundColor Green
Write-Host '  .\scripts\play.ps1 -Game "D:\path\to\your\dump"'

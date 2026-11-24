# Build a portable Neonbridge release zip.
# Builds into target\package (never touches target\release), then runs
# neonbridge --package-release, which builds the TuxBox providers and xshader
# and lays out neonbridge-<version>-win-x64\ plus its .zip in target\package\dist.
param([string]$Out = "$PSScriptRoot\..\target\package\dist")
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\.."
cargo build --release --locked --manifest-path "$root\Cargo.toml" --target-dir "$root\target\package"
if ($LASTEXITCODE -ne 0) { throw "cargo build of neonbridge failed ($LASTEXITCODE)" }
& "$root\target\package\release\neonbridge.exe" --package-release $Out
if ($LASTEXITCODE -ne 0) { throw "packaging failed ($LASTEXITCODE)" }

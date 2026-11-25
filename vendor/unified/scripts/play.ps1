# Unified launcher (fxceless 0b0074e + Belobug hoist/ambient/quads + RTX 3070 tuned)
param(
    [Parameter(Mandatory = $true)][string]$Game,
    [string]$Cache = "$env:LOCALAPPDATA\ztrx\cache",
    [string]$Data = "$env:LOCALAPPDATA\ztrx\data",
    [string]$Exe = ""
)
if (-not $Exe) {
    $ScriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Definition }
    $Exe = Join-Path (Split-Path -Parent $ScriptDir) "target\release\victory.exe"
}
foreach ($v in "VICTORY_KEYS", "VICTORY_CAPTURE", "VICTORY_CAPTURE_FRAMES", "VICTORY_DUMP_TARGETS", "VICTORY_INPLACE", "VICTORY_INPLACE_MB") {
    Remove-Item "Env:$v" -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Force $Cache, $Data | Out-Null
$env:VICTORY_CACHE_DIR = $Cache
$env:VICTORY_DATA_DIR = $Data
# Performance pipeline (fxceless async submit + copy queue2 + DexNom range-lock streaming)
$env:VICTORY_ASYNC_SUBMIT = "1"
$env:VICTORY_QUEUE2 = "1"
$env:VICTORY_QUEUE2_LANDING_MB = "128"
$env:VICTORY_REFRESH_ONCE = "1"
$env:VICTORY_COPY_PLAIN = "1"
$env:VICTORY_CARD_BUFFERS_MIN_MB = "1"
$env:VICTORY_CARD_BUFFERS_MB = "768"
$env:VICTORY_MERGE_GAP = "64"
$env:VICTORY_MERGE_RUNS = "4096"
$env:VICTORY_JOIN_COPIES = "1"
$env:VICTORY_CARD_REHASH_RUNS = "1"
$env:VICTORY_VIEW_KNOWN = "1"
$env:VICTORY_UNWATCHED_FRAMES = "4"
$env:VICTORY_DISK_CORES = "slow"
$env:VICTORY_PIPELINE_WAIT_MS = "24"
# Graphics, lighting, on-demand mip streaming & stability (RTX 3070 8 GB tuned)
$env:VICTORY_AMBIENT = "1.25"
$env:VICTORY_MAX_FPS = "30"
$env:VICTORY_HOIST_CLAMPS = "7"
$env:VICTORY_INDIRECT_WINDOW = "0"
$env:VICTORY_LATE_WAIT_LISTS = "64"
$env:VICTORY_LEVELS_ON_DEMAND = "1"
$env:VICTORY_TEXTURE_NO_WAIT = "1"
$env:VICTORY_TEXTURE_BUDGET_MB = "1920"
$env:VICTORY_TEXTURE_STAGE_MB = "192"
$env:VICTORY_TEXTURE_STAGE_MS = "120"
$env:RUST_BACKTRACE = "1"
$env:VICTORY_LOG = "info"
& $Exe run $Game --execute 2>&1 | Out-File "$env:TEMP\ztrx.log" -Encoding utf8
Write-Host "Exited with $LASTEXITCODE; log: $env:TEMP\ztrx.log"

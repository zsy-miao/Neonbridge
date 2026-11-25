# Launches a title on the VIctory-based runtime from your own dump (you need your own legally
# obtained copy; nothing is included in this repository).
# Usage: .\scripts\play-victory.ps1 -Game "D:\path\to\dump" [-AutoInput] [-Console series-s|series-x]
#          [-NoOwnUpscaler] [-KeepWatchdog] [-FastBoot] [-SaveSlot name] [-Precompile] [-SkipPrecompile]
#
# Environment it sets (you can also set these by hand before running the exe):
#   VICTORY_GAME_DIR       the dump folder
#   VICTORY_OWN_UPSCALER=1 ask for the title's own upscaler on the Series S profile (recommended)
#   VICTORY_NO_WATCHDOG=1  disarm the title's stall watchdog so it can wait for slow first-run
#                          shader builds instead of ending itself
#   VICTORY_SYNC_PRECISE=1 a submission waits only for the submissions it depends on
#   VICTORY_READ_SETTLE_PRECISE=1  read-backs settle on the exact submissions they need
#   VICTORY_PRECOMPILED_GRAPHICS=1 graphics pipelines come from the title's precompiled cache,
#                          prebuilt by `victory precompile-graphics` (run once, below; needs a GPU)
#   VICTORY_READFIX, VICTORY_COPY_BACK_ONLY_READBACK, VICTORY_NARROW_ROOT / _VIEWS / _BVH,
#                          VICTORY_PREP_BARRIERS / _TABLES / _REUSE = 1: the other verified speed settings
#   VICTORY_RELAY=2        submission relay, form 2 (needed by the two settings above)
#   VICTORY_PIPELINE_WAIT_MS=0  do not hold a draw back waiting for a pipeline build
#   VICTORY_FAST_BOOT=1    with -FastBoot: press through logos and menus (testing only)
#   VICTORY_SAVE_SLOT      with -SaveSlot: pick a save slot by name
#   VICTORY_AUTO_INPUT=1   with -AutoInput: press A on a timer so menus advance without a pad
#   VICTORY_PRESS_FILE     press.txt in the repo root: write button names ("A", "DOWN DOWN A") to press them
param(
    [Parameter(Mandatory)] [string]$Game,
    # The console profile the title sees. series-s is the recommended default.
    [ValidateSet('series-s', 'series-x')] [string]$Console = 'series-s',
    # Press A on a timer so the title screens advance without a controller.
    [switch]$AutoInput,
    # Do not set VICTORY_OWN_UPSCALER=1.
    [switch]$NoOwnUpscaler,
    # Do not set VICTORY_NO_WATCHDOG=1.
    [switch]$KeepWatchdog,
    # Testing only: VICTORY_FAST_BOOT=1.
    [switch]$FastBoot,
    # Save slot name (VICTORY_SAVE_SLOT).
    [string]$SaveSlot = '',
    # Run `victory precompile-graphics` even if it already ran once.
    [switch]$Precompile,
    # Do not run `victory precompile-graphics` (pipelines are then built while you play).
    [switch]$SkipPrecompile
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$vic  = Join-Path $root 'runtime\victory\target\release\victory.exe'

if (-not (Test-Path $vic)) { throw 'Not built yet. Run .\scripts\setup-victory.ps1 first.' }
$exe = Join-Path $Game 'game_scarlett_master.exe'
if (-not (Test-Path $exe)) {
    throw "No game_scarlett_master.exe in '$Game'. Point -Game at your own dump."
}

$env:VICTORY_GAME_DIR = $Game
if (-not $NoOwnUpscaler) { $env:VICTORY_OWN_UPSCALER = '1' }
if (-not $KeepWatchdog) { $env:VICTORY_NO_WATCHDOG = '1' }
# The verified speed settings (see docs/RUNTIMES.md). The experimental memory switches
# (msfix, mem2) stay off.
$env:VICTORY_RELAY = '2'
$env:VICTORY_SYNC_PRECISE = '1'
$env:VICTORY_READ_SETTLE_PRECISE = '1'
# The rest of the verified speed settings. The grid fix is in this runtime, so precompiled graphics pipelines are on.
foreach ($k in 'VICTORY_PRECOMPILED_GRAPHICS','VICTORY_READFIX','VICTORY_COPY_BACK_ONLY_READBACK','VICTORY_NARROW_ROOT','VICTORY_NARROW_VIEWS','VICTORY_NARROW_BVH','VICTORY_PREP_BARRIERS','VICTORY_PREP_TABLES','VICTORY_PREP_REUSE') { Set-Item "env:$k" '1' }
$env:VICTORY_PIPELINE_WAIT_MS = '0'
if ($FastBoot) { $env:VICTORY_FAST_BOOT = '1' }
if ($SaveSlot) { $env:VICTORY_SAVE_SLOT = $SaveSlot }
# Write button names here ("A", "DOWN DOWN A") to press them while the game runs.
$env:VICTORY_PRESS_FILE = Join-Path $root 'press.txt'
Set-Content -Path $env:VICTORY_PRESS_FILE -Value '' -NoNewline
if ($AutoInput) { $env:VICTORY_AUTO_INPUT = '1' }

# Prebuild the title's precompiled graphics pipelines into the pipeline library (needs a GPU; the
# first run takes a while and is done once, marked by a stamp file).
$stamp = Join-Path $root 'logs\precompile-graphics.done'
if (-not $SkipPrecompile -and ($Precompile -or -not (Test-Path $stamp))) {
    Write-Host 'Building the title''s precompiled graphics pipelines (once)...' -ForegroundColor Cyan
    $ErrorActionPreference = 'Continue'
    & $vic precompile-graphics
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw "victory precompile-graphics failed (exit $code)." }
    New-Item -ItemType Directory -Force (Split-Path $stamp) | Out-Null
    Set-Content -Path $stamp -Value (Get-Date -Format s)
}

$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force $logs | Out-Null
$log = Join-Path $logs ("victory-{0:yyyyMMdd-HHmmss}.log" -f (Get-Date))
Write-Host "Starting on the $Console profile... log: $log" -ForegroundColor Cyan
# Windows PowerShell 5.1 turns a native program's stderr into a terminating error under 'Stop'
# when it is redirected, which would kill the run on the first log line.
$ErrorActionPreference = 'Continue'
& $vic run $exe --execute --console $Console *> $log
exit $LASTEXITCODE

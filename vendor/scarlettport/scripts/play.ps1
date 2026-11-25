# Launches the game from your own unpacked Xbox Series dump.
# Usage: .\scripts\play.ps1 -Game "D:\path\to\dump" [-AutoMenu]
param(
    [Parameter(Mandatory)] [string]$Game,
    # Press A every 6 s from 22 s to 70 s: walks the account picker and title screens hands-free.
    [switch]$AutoMenu
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$xcl  = Join-Path $root 'runtime\xcl\target\release\xcl.exe'

if (-not (Test-Path $xcl)) { throw 'Not built yet. Run .\scripts\setup.ps1 first.' }
if (-not (Test-Path (Join-Path $Game 'game_scarlett_master.exe'))) {
    throw "No game_scarlett_master.exe in '$Game'. Point -Game at your unpacked dump."
}

# Fewer parallel shader compiles: 32 GB machines run out of memory with the default.
$env:XCL_DXC_THREADS = '4'
# Turn off the game's 30 s stall watchdog so it waits for slow first-time shader builds
# instead of killing itself (ivall/xcl#505).
$env:XCL_NO_WATCHDOG = '1'
# Write button names here ("A", "DOWN DOWN A") to press them while the game runs.
$env:XCL_PRESS_FILE = Join-Path $root 'press.txt'
Set-Content -Path $env:XCL_PRESS_FILE -Value '' -NoNewline
if ($AutoMenu) { $env:XCL_PRESS_AT = ((22..70 | Where-Object { ($_ - 22) % 6 -eq 0 }) | ForEach-Object { "${_}:A" }) -join ',' }

$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force $logs | Out-Null
$log = Join-Path $logs ("run-{0:yyyyMMdd-HHmmss}.log" -f (Get-Date))
Write-Host "Starting... log: $log" -ForegroundColor Cyan
# Windows PowerShell 5.1 turns a native program's stderr into a terminating error under 'Stop'
# when it is redirected, which would kill the run on the first log line.
$ErrorActionPreference = 'Continue'
& $xcl run $Game *> $log
exit $LASTEXITCODE

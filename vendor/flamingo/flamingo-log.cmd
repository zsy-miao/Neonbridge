@echo off
rem Flamingo logging run: builds Flamingo, runs it with its log on, and saves everything it
rem says, with your PC's details, to a log file (and a .zip of it) to attach to a GitHub issue.
rem
rem Use: drag the title's executable from your own dump onto this file, or run
rem   flamingo-log.cmd "D:\path\to\your\dump\the-title.exe"
rem The path is remembered in flamingo-game.txt for the next time.
rem Series X profile instead of Series S: set FLAMINGO_SERIES_X=1 before running.
rem FLAMINGO_LOG_QUIET=1: no pause and no Explorer window at the end (for scripts).
setlocal
cd /d "%~dp0"

set "GAME=%~1"
if "%GAME%"=="" if exist flamingo-game.txt set /p GAME=<flamingo-game.txt
if "%GAME%"=="" goto :usage
if not exist "%GAME%" goto :notfound
> flamingo-game.txt echo %GAME%

where cargo >nul 2>&1
if errorlevel 1 goto :norust
echo Building Flamingo (the first build takes several minutes)...
cargo build --profile diag -p victory
if errorlevel 1 goto :buildfailed

set "TARGET=target"
if defined CARGO_TARGET_DIR set "TARGET=%CARGO_TARGET_DIR%"
if not exist logs mkdir logs
for /f %%t in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd-HHmmss"') do set "STAMP=%%t"
set "LOG=%~dp0logs\flamingo-%STAMP%.log"

rem What the log starts with: the build and the PC.
> "%LOG%" echo Flamingo logging run %STAMP%
for /f %%c in ('git rev-parse --short HEAD 2^>nul') do >> "%LOG%" echo commit %%c
powershell -NoProfile -Command "$os = Get-CimInstance Win32_OperatingSystem; $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1; 'Windows: ' + $os.Caption + ' ' + $os.Version; 'CPU: ' + $cpu.Name.Trim(); 'RAM: ' + [math]::Round($os.TotalVisibleMemorySize / 1MB, 1) + ' GB'; Get-CimInstance Win32_VideoController | ForEach-Object { 'GPU: ' + $_.Name + ', driver ' + $_.DriverVersion }" >> "%LOG%" 2>&1

rem The settings the README gives, with the log on.
set NO_COLOR=1
set VICTORY_LOG=info
set VICTORY_NO_WATCHDOG=1
set VICTORY_PIPELINE_WAIT_MS=24
set VICTORY_RELAY=1
set VICTORY_LATE_WAIT=3
set VICTORY_CARD_BUFFERS_MIN_MB=1
set VICTORY_CARD_BUFFERS_MB=1024
set VICTORY_INDIRECT_WINDOW=1
set VICTORY_SHADER_OPT=0
set VICTORY_WIDE_MERGE=1
set VICTORY_WIDE_TEXTURES=1
set VICTORY_VIEW_GUARD=0
set FLAMINGO_BVH_MISS=1
set FLAMINGO_OSD=1
set "CONSOLE=series-s"
if "%FLAMINGO_SERIES_X%"=="1" set "CONSOLE=series-x"
>> "%LOG%" echo profile %CONSOLE%

echo.
echo Running. Play until the problem shows (for a black screen: wait a minute in it),
echo then close the game window. The log is written to:
echo   %LOG%
echo.
"%TARGET%\diag\victory.exe" run "%GAME%" --execute --console %CONSOLE% >> "%LOG%" 2>&1
>> "%LOG%" echo exit code %ERRORLEVEL%

powershell -NoProfile -Command "Compress-Archive -Force -LiteralPath '%LOG%' -DestinationPath '%LOG%.zip'"
echo.
echo Done. Attach this file to your GitHub issue:
echo   %LOG%.zip
echo The log holds file paths of this PC (they can include your Windows user name):
echo look it over before you share it.
if not "%FLAMINGO_LOG_QUIET%"=="1" explorer /select,"%LOG%.zip"
goto :end

:usage
echo Drag the title's executable from your own dump onto this file, or run:
echo   flamingo-log.cmd "D:\path\to\your\dump\the-title.exe"
goto :end

:notfound
echo Not found: "%GAME%"
goto :end

:norust
echo Rust is not installed. Install it from https://rustup.rs and run this again.
goto :end

:buildfailed
echo The build failed. Please copy the text above into your issue.
goto :end

:end
if not "%FLAMINGO_LOG_QUIET%"=="1" pause
endlocal

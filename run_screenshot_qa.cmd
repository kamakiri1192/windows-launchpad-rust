@echo off
setlocal

cd /d "%~dp0"

set "EXE=%~dp0target\release\launchpad-windows.exe"

if not exist "%EXE%" (
    where cargo >nul 2>nul
    if errorlevel 1 (
        echo Rust Cargo was not found on PATH.
        echo Install Rust or open this from a shell where cargo is available.
        pause
        exit /b 1
    )

    echo Release binary was not found. Building it now...
    cargo build --release --locked
    if errorlevel 1 (
        echo.
        echo Release build failed.
        pause
        exit /b 1
    )
)

tasklist /FI "IMAGENAME eq launchpad-windows.exe" 2>nul | find /I "launchpad-windows.exe" >nul
if not errorlevel 1 (
    echo Launchpad is already running.
    echo Quit it from the tray icon first, then run this file again so the screenshot setting takes effect.
    pause
    exit /b 1
)

set "LAUNCHPAD_ALLOW_SCREENSHOT=1"
set "LAUNCHPAD_DEBUG=1"

echo Starting Launchpad with screenshot capture enabled.
echo Debug log: %%LOCALAPPDATA%%\Launchpad\debug.log
echo.

start "" "%EXE%" %*
if errorlevel 1 (
    echo Launchpad failed to start.
    pause
    exit /b 1
)

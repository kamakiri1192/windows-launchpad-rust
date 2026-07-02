@echo off
setlocal

cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo Rust Cargo was not found on PATH.
    echo Install Rust or open this from a shell where cargo is available.
    pause
    exit /b 1
)

cargo run --bin liquid_glass_studio --locked
if errorlevel 1 (
    echo.
    echo Liquid Glass Studio failed to start.
    pause
    exit /b 1
)

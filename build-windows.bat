@echo off
setlocal

set PROJECT_DIR=%~dp0
set TAURI_DIR=%PROJECT_DIR%src-tauri
set OUT_DIR=%PROJECT_DIR%dist

if not exist "%OUT_DIR%" mkdir "%OUT_DIR%"

echo === Checking build prerequisites ===
cmake --version >nul 2>&1
if %ERRORLEVEL% neq 0 (
    echo ERROR: cmake not found. Install from https://cmake.org/download/
    echo        and ensure it is on your PATH.
    pause
    exit /b 1
)

echo === Building Windows x86_64 ===
cd /d "%PROJECT_DIR%"

cargo tauri build 2>&1
if %ERRORLEVEL% neq 0 (
    echo BUILD FAILED
    pause
    exit /b 1
)

copy /y "%TAURI_DIR%\target\release\live-meeting-helper.exe" "%OUT_DIR%\live-meeting-helper-windows-x86_64.exe"
echo   → %OUT_DIR%\live-meeting-helper-windows-x86_64.exe
echo === Build complete ===
pause

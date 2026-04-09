@echo off
setlocal

set PROJECT_DIR=%~dp0

echo === Checking build prerequisites ===
cmake --version >nul 2>&1
if %ERRORLEVEL% neq 0 (
    echo ERROR: cmake not found. Install from https://cmake.org/download/
    echo        and ensure it is on your PATH.
    pause
    exit /b 1
)

echo === Dev Build (debug, fast compile) ===
cd /d "%PROJECT_DIR%"

cargo tauri build --debug 2>&1
if %ERRORLEVEL% neq 0 (
    echo BUILD FAILED
    pause
    exit /b 1
)

set OUT_DIR=%PROJECT_DIR%dist
if not exist "%OUT_DIR%" mkdir "%OUT_DIR%"
copy /y "%PROJECT_DIR%src-tauri\target\debug\live-meeting-helper.exe" "%OUT_DIR%\live-meeting-helper-dev.exe"
echo   → %OUT_DIR%\live-meeting-helper-dev.exe
echo === Dev build complete ===
pause

@echo off
setlocal
cd /d "%~dp0"

if not exist "target\release\playground.exe" (
    echo Building Playground for the first launch...
    where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    cargo build --release
    if errorlevel 1 (
        echo.
        echo The build failed. Review the errors above, then try again.
        pause
        exit /b 1
    )
)

start "" "%~dp0target\release\playground.exe"

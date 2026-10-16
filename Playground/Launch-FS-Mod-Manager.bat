@echo off
setlocal
cd /d "%~dp0"

if not exist "target\release\fs-mod-manager.exe" (
    echo Building FS Mod Manager for the first launch...
    where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    cargo build --release
    if errorlevel 1 (
        echo.
        echo The build failed. Review the errors above, then try again.
        pause
        exit /b 1
    )
)

start "" "%~dp0target\release\fs-mod-manager.exe"

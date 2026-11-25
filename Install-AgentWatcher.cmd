@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\Setup.ps1" %*
set "EXIT_CODE=%errorlevel%"
if %EXIT_CODE% neq 0 (
    echo.
    echo Agent Watcher setup failed with exit code %EXIT_CODE%.
) else (
    echo.
)
echo %CMDCMDLINE% | find /i "/c" >nul
if not errorlevel 1 if "%~1"=="" pause
exit /b %EXIT_CODE%

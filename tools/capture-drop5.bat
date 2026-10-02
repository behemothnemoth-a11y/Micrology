@echo off
REM Double-click this file to record the DROP 0005 collapse demo.
REM
REM It calls capture-demo.ps1 with -ExecutionPolicy Bypass, because Windows
REM blocks unsigned local PowerShell scripts by default and that block is the
REM first thing anyone hits.

cd /d "%~dp0\.."

echo.
echo  Micrology - DROP 0005 collapse capture
echo  ======================================
echo.
echo  Two identical towers, brick and steel. Only the brick one falls.
echo.
echo  Checking capture prerequisites first...
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0capture-demo.ps1" -CheckOnly
if errorlevel 1 (
    echo.
    echo  Capture prerequisites are missing. Fix the error above and rerun.
    echo.
    pause
    exit /b 1
)
echo.
echo  This will build the sandbox (slow the first time), open a window,
echo  record about 20 seconds, then close itself.
echo.
echo  The capture tool will not move or minimize your other windows.
echo  Keep the Micrology window unobstructed while it records.
echo.
pause

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0capture-demo.ps1" -Demo collapse -Section drop0005_hardware

echo.
echo  Done. The video and thumbnail are in:
echo    docs\diagnostics\drop0005_hardware\
echo.
echo  Open thumbnail.png and check it is not blank white.
echo.
pause

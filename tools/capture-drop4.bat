@echo off
REM Double-click this file to record the DROP 0004.9 hardware acceptance.

cd /d "%~dp0\.."

echo.
echo  Micrology - DROP 0004.9 hardware acceptance
echo  ==========================================
echo.
echo  The full DROP 4 loop: damage, detach, impulse, re-fracture,
echo  secondary damage.
echo.
echo  This will build the sandbox (slow the first time), open a window,
echo  record about 20 seconds, then close itself.
echo.
echo  Do not click anything while it records.
echo.
pause

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0capture-demo.ps1" -Demo impact -Section drop0004_9_hardware

echo.
echo  Done. The video and thumbnail are in:
echo    docs\diagnostics\drop0004_9_hardware\
echo.
echo  Open thumbnail.png and check it is not blank white.
echo.
pause

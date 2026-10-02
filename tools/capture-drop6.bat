@echo off
setlocal
cd /d "%~dp0.."

echo.
echo  Micrology DROP 0006 fracture capture
echo  --------------------------------------
echo  Checking FFmpeg / ddagrab first...
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
echo  This capture is non-invasive. It will not move or minimize other windows.
echo  Keep the Micrology window unobstructed while the fracture demo records.
echo.
pause

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0capture-demo.ps1" -Demo fracture -Section drop0006_0_fracture

echo.
if errorlevel 1 (
    echo  Capture failed. See the error above.
) else (
    echo  Capture complete: docs\diagnostics\drop0006_0_fracture
)
echo.
pause

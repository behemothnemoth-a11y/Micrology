<#
.SYNOPSIS
    Record a fresh GPU capture of a Micrology sandbox demo.

.DESCRIPTION
    Every acceptance visual since DROP 0004.7 has been produced by hand, which
    is why the capture keeps being the thing that blocks a section from closing.
    This script is that procedure, committed.

    It uses FFmpeg's Desktop Duplication filter (ddagrab). Ordinary gdigrab
    finds the window but records the Vulkan surface as blank white, so it is
    not an option here.

.PARAMETER Demo
    Which demo to seed:
      impact      - the DROP 0004.8 loop: damage, detach, impulse, re-fracture
      progressive - DROP 0004.7 accumulating local damage
      collapse    - DROP 0005 material mechanics: two identical towers, brick
                    and steel, where only the brick one falls

.PARAMETER Section
    Diagnostics folder to write into, e.g. drop0004_9_hardware.

.PARAMETER Seconds
    Recording length. The demos finish well inside 20 seconds.

.PARAMETER Warmup
    Seconds to wait after launch before recording, so shader compilation and
    the first streaming pass are not in frame.

.EXAMPLE
    .\tools\capture-demo.ps1 -Demo impact -Section drop0004_9_hardware
    .\tools\capture-demo.ps1 -Demo collapse -Section drop0005_hardware
#>
[CmdletBinding()]
param(
    [ValidateSet('impact', 'progressive', 'collapse')]
    [string]$Demo = 'impact',

    [string]$Section = 'drop0004_9_hardware',

    [int]$Seconds = 20,

    [int]$Warmup = 8,

    [int]$Framerate = 30,

    [switch]$KeepRaw
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$windowTitle = 'Micrology — DROP 0004 sandbox'

# --- Preflight -------------------------------------------------------------

function Require-Command([string]$name, [string]$hint) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "$name not found on PATH. $hint"
    }
}

Require-Command 'cargo'  'Install the Rust toolchain.'
Require-Command 'ffmpeg' 'Install FFmpeg (winget install Gyan.FFmpeg) and reopen the terminal.'

# ddagrab ships only in reasonably recent Windows builds of FFmpeg. Fail here
# with a clear reason rather than producing a blank-white file later.
$filters = & ffmpeg -hide_banner -filters 2>&1 | Out-String
if ($filters -notmatch 'ddagrab') {
    throw @'
This FFmpeg has no ddagrab filter, so it cannot capture a Vulkan surface.
Install a recent Windows build (winget install Gyan.FFmpeg) and try again.
Do not fall back to gdigrab: it records the Micrology window as blank white.
'@
}

$envName = switch ($Demo) {
    'impact'      { 'MICROLOGY_IMPACT_DEMO' }
    'progressive' { 'MICROLOGY_PROGRESSIVE_DAMAGE_DEMO' }
    'collapse'    { 'MICROLOGY_COLLAPSE_DEMO' }
}

$outDir = Join-Path $repo "docs/diagnostics/$Section"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$stamp     = Get-Date -Format 'yyyyMMdd-HHmmss'
$rawPath   = Join-Path $env:TEMP "micrology-capture-$stamp.mp4"
$videoName = "Micrology_$($Section)_$($Demo).mp4"
$videoPath = Join-Path $outDir $videoName
$thumbPath = Join-Path $outDir 'thumbnail.png'

# --- Win32 window control --------------------------------------------------

if (-not ('Win32Window' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Win32Window {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    public const int SW_RESTORE  = 9;
    public const int SW_MAXIMIZE = 3;
}
'@
}

# --- Build -----------------------------------------------------------------

Write-Host "Building sandbox (release)..." -ForegroundColor Cyan
Push-Location $repo
try {
    & cargo build --release -p sandbox
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

# --- Launch ----------------------------------------------------------------

Write-Host "Launching with $envName=1..." -ForegroundColor Cyan
$exe = Join-Path $repo 'target/release/sandbox.exe'
if (-not (Test-Path $exe)) { throw "Built binary not found at $exe" }

$previous = [Environment]::GetEnvironmentVariable($envName)
[Environment]::SetEnvironmentVariable($envName, '1')
try {
    $proc = Start-Process -FilePath $exe -WorkingDirectory $repo -PassThru
} finally {
    [Environment]::SetEnvironmentVariable($envName, $previous)
}

try {
    # Wait for the window to actually exist before touching it.
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $proc.MainWindowHandle -or $proc.MainWindowHandle -eq [IntPtr]::Zero) {
        if ((Get-Date) -gt $deadline) { throw 'Sandbox window never appeared within 60s.' }
        Start-Sleep -Milliseconds 250
        $proc.Refresh()
    }

    $handle = $proc.MainWindowHandle
    if ([Win32Window]::IsIconic($handle)) {
        [Win32Window]::ShowWindow($handle, [Win32Window]::SW_RESTORE) | Out-Null
    }
    [Win32Window]::ShowWindow($handle, [Win32Window]::SW_MAXIMIZE) | Out-Null
    [Win32Window]::SetForegroundWindow($handle) | Out-Null

    Write-Host "Warming up for ${Warmup}s (shader compile, first stream pass)..." -ForegroundColor Cyan
    Start-Sleep -Seconds $Warmup

    # --- Record ------------------------------------------------------------
    # ddagrab hands back GPU frames; hwdownload+format brings them to system
    # memory so a normal encoder can take them.
    Write-Host "Recording ${Seconds}s with ddagrab..." -ForegroundColor Cyan
    & ffmpeg -hide_banner -loglevel warning -y `
        -init_hw_device d3d11va `
        -filter_complex "ddagrab=framerate=${Framerate}:draw_mouse=0,hwdownload,format=bgra" `
        -t $Seconds `
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p `
        $rawPath
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg capture failed with exit code $LASTEXITCODE" }
}
finally {
    if ($proc -and -not $proc.HasExited) {
        Write-Host 'Closing sandbox...' -ForegroundColor DarkGray
        $proc.CloseMainWindow() | Out-Null
        Start-Sleep -Seconds 2
        if (-not $proc.HasExited) { $proc | Stop-Process -Force }
    }
}

# --- Trim the taskbar ------------------------------------------------------
# Captured at full desktop size; crop the bottom strip so the taskbar is not in
# the committed visual. Everything above it is the live Vulkan surface.

Write-Host 'Cropping...' -ForegroundColor Cyan
& ffmpeg -hide_banner -loglevel warning -y -i $rawPath `
    -vf "crop=iw:ih-48:0:0" `
    -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p `
    $videoPath
if ($LASTEXITCODE -ne 0) { throw "ffmpeg crop failed with exit code $LASTEXITCODE" }

# A frame from late in the run, when the demo has reported its totals.
& ffmpeg -hide_banner -loglevel warning -y -sseof -3 -i $videoPath -vframes 1 $thumbPath
if ($LASTEXITCODE -ne 0) { throw "ffmpeg thumbnail failed with exit code $LASTEXITCODE" }

if (-not $KeepRaw) { Remove-Item $rawPath -ErrorAction SilentlyContinue }

$size = [math]::Round((Get-Item $videoPath).Length / 1MB, 2)
Write-Host ''
Write-Host "Video     $videoPath  (${size} MB)" -ForegroundColor Green
Write-Host "Thumbnail $thumbPath" -ForegroundColor Green
Write-Host ''
Write-Host 'Check the thumbnail before committing: if it is blank white, the' -ForegroundColor Yellow
Write-Host 'capture did not get the Vulkan surface and the run is not valid.'  -ForegroundColor Yellow

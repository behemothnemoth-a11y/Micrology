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

.PARAMETER CheckOnly
    Verify Rust, FFmpeg, ffprobe, and the ddagrab filter, then exit without
    building, launching Micrology, or touching any desktop windows.

.EXAMPLE
    .\tools\capture-demo.ps1 -CheckOnly
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

    [switch]$KeepRaw,

    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

# --- Preflight -------------------------------------------------------------

function Require-Command([string]$name, [string]$hint) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "$name not found on PATH. $hint"
    }
}

Require-Command 'cargo'   'Install the Rust toolchain.'
Require-Command 'ffmpeg'  'Install FFmpeg (winget install Gyan.FFmpeg) and reopen the terminal.'
Require-Command 'ffprobe' 'Install FFmpeg (winget install Gyan.FFmpeg) and reopen the terminal.'

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

if ($CheckOnly) {
    $cargoVersion = (& cargo --version 2>&1 | Select-Object -First 1)
    $ffmpegVersion = (& ffmpeg -hide_banner -version 2>&1 | Select-Object -First 1)
    $ffmpegPath = (Get-Command ffmpeg).Source
    $ffprobePath = (Get-Command ffprobe).Source
    Write-Host ''
    Write-Host 'Micrology capture preflight: PASS' -ForegroundColor Green
    Write-Host "  Rust     $cargoVersion"
    Write-Host "  FFmpeg   $ffmpegVersion"
    Write-Host "  ffmpeg   $ffmpegPath"
    Write-Host "  ffprobe  $ffprobePath"
    Write-Host '  ddagrab  available (required for Vulkan capture)'
    Write-Host ''
    Write-Host 'No build was run and no desktop windows were changed.' -ForegroundColor DarkGray
    exit 0
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

# --- Window geometry (read-only) ------------------------------------------
#
# Capture must not rearrange Justin's desktop. The only Win32 calls here read
# the Micrology window state/rectangle so the full-desktop ddagrab frame can be
# cropped afterward. No minimize, restore, resize, move, topmost, or foreground
# operations belong in this script.

Add-Type -AssemblyName System.Windows.Forms

if (-not ('Win32Window' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public class Win32Window {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
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
Write-Host 'Capture is non-invasive: no other desktop windows will be moved or minimized.' -ForegroundColor DarkGray
Write-Host 'Keep the Micrology window unobstructed while recording.' -ForegroundColor Yellow
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
    # Wait for Micrology to create its native window. We only observe it; capture
    # never changes focus, size, position, z-order, or any other desktop window.
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $proc.MainWindowHandle -or $proc.MainWindowHandle -eq [IntPtr]::Zero) {
        if ((Get-Date) -gt $deadline) { throw 'Sandbox window never appeared within 60s.' }
        Start-Sleep -Milliseconds 250
        $proc.Refresh()
    }

    Write-Host "Warming up for ${Warmup}s (shader compile, first stream pass)..." -ForegroundColor Cyan
    Start-Sleep -Seconds $Warmup
    $proc.Refresh()
    if ($proc.HasExited) {
        throw "Sandbox exited during the ${Warmup}s warm-up; reduce -Warmup or lengthen the demo hold."
    }

    # Winit may recreate its native window during renderer initialization. Read
    # the final handle after warm-up and use it only for validation/cropping.
    $handle = $proc.MainWindowHandle
    if (-not $handle -or $handle -eq [IntPtr]::Zero) {
        throw 'Sandbox lost its window handle before recording.'
    }
    if ([Win32Window]::IsIconic($handle)) {
        throw 'Micrology is minimized. Restore it and rerun; this script will not alter your desktop windows.'
    }
    $windowRect = New-Object Win32Window+RECT
    if (-not [Win32Window]::GetWindowRect($handle, [ref]$windowRect)) {
        throw 'Could not read the Micrology window rectangle before capture.'
    }

    $windowWidth = $windowRect.Right - $windowRect.Left
    $windowHeight = $windowRect.Bottom - $windowRect.Top
    Write-Host "Micrology window: ${windowWidth}x${windowHeight} at $($windowRect.Left),$($windowRect.Top)" -ForegroundColor DarkGray
    Write-Host "Recording ${Seconds}s with ddagrab..." -ForegroundColor Cyan
    & ffmpeg -hide_banner -loglevel warning -y `
        -init_hw_device d3d11va `
        -filter_complex "ddagrab=framerate=${Framerate}:draw_mouse=0,hwdownload,format=bgra" `
        -t $Seconds `
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p `
        $rawPath
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg capture failed with exit code $LASTEXITCODE" }
    $proc.Refresh()
    if ($proc.HasExited) {
        throw 'Sandbox exited before recording completed; reduce -Seconds or lengthen the demo hold.'
    }
}
finally {
    if ($proc -and -not $proc.HasExited) {
        Write-Host 'Closing sandbox...' -ForegroundColor DarkGray
        $proc.CloseMainWindow() | Out-Null
        Start-Sleep -Seconds 2
        if (-not $proc.HasExited) { $proc | Stop-Process -Force }
    }
}

# --- Crop to the Micrology window ----------------------------------------
# ddagrab captures physical desktop pixels while Win32 window rectangles are in
# logical desktop coordinates on a DPI-scaled display. Convert the stable
# post-warm-up window rectangle into physical capture coordinates and crop to the
# Micrology window itself, rather than committing the surrounding desktop.
$screen = [System.Windows.Forms.Screen]::PrimaryScreen
$probe = (& ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 $rawPath).Trim()
if ($probe -notmatch '^(\d+)x(\d+)$') { throw "could not read capture dimensions: $probe" }
$rawWidth = [int]$Matches[1]
$rawHeight = [int]$Matches[2]
$scaleX = $rawWidth / [double]$screen.Bounds.Width
$scaleY = $rawHeight / [double]$screen.Bounds.Height
$cropX = [int][math]::Round(($windowRect.Left - $screen.Bounds.X) * $scaleX)
$cropY = [int][math]::Round(($windowRect.Top - $screen.Bounds.Y) * $scaleY)
$cropWidth = [int][math]::Round(($windowRect.Right - $windowRect.Left) * $scaleX)
$cropHeight = [int][math]::Round(($windowRect.Bottom - $windowRect.Top) * $scaleY)
$cropX = [math]::Max(0, [math]::Min($cropX, $rawWidth - 2))
$cropY = [math]::Max(0, [math]::Min($cropY, $rawHeight - 2))
$cropWidth = [math]::Min($cropWidth, $rawWidth - $cropX)
$cropHeight = [math]::Min($cropHeight, $rawHeight - $cropY)
# H.264 yuv420p requires even dimensions and offsets.
$cropX -= $cropX % 2
$cropY -= $cropY % 2
$cropWidth -= $cropWidth % 2
$cropHeight -= $cropHeight % 2
if ($cropWidth -le 0 -or $cropHeight -le 0) {
    throw "invalid Micrology window crop ${cropWidth}x${cropHeight}+${cropX}+${cropY}"
}

Write-Host "Cropping Micrology window ${cropWidth}x${cropHeight}+${cropX}+${cropY} from ${rawWidth}x${rawHeight} capture..." -ForegroundColor Cyan
& ffmpeg -hide_banner -loglevel warning -y -i $rawPath `
    -vf "crop=${cropWidth}:${cropHeight}:${cropX}:${cropY}" `
    -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p `
    $videoPath
if ($LASTEXITCODE -ne 0) { throw "ffmpeg crop failed with exit code $LASTEXITCODE" }

# A frame from late in the run, when the demo has reported its totals.
& ffmpeg -hide_banner -loglevel warning -y -sseof -3 -i $videoPath -frames:v 1 -update 1 $thumbPath
if ($LASTEXITCODE -ne 0) { throw "ffmpeg thumbnail failed with exit code $LASTEXITCODE" }

if (-not $KeepRaw) { Remove-Item $rawPath -ErrorAction SilentlyContinue }

$size = [math]::Round((Get-Item $videoPath).Length / 1MB, 2)
Write-Host ''
Write-Host "Video     $videoPath  (${size} MB)" -ForegroundColor Green
Write-Host "Thumbnail $thumbPath" -ForegroundColor Green
Write-Host ''
Write-Host 'Check the thumbnail before committing: if it is blank white, the' -ForegroundColor Yellow
Write-Host 'capture did not get the Vulkan surface and the run is not valid.'  -ForegroundColor Yellow

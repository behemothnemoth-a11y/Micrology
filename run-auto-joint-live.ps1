$ErrorActionPreference='Stop'
$repo='C:\Users\behem\.chatgpt-worktrees\Micrology-house-joint-load'
Set-Location $repo
$diag=Join-Path $repo 'target\diagnostics\destruction_lab'
New-Item -ItemType Directory -Force -Path $diag | Out-Null
Get-ChildItem $diag -Filter 'autoj_*' -ErrorAction SilentlyContinue | Remove-Item -Force
$env:MICROLOGY_REFERENCE_HOUSE='1'
$env:MICROLOGY_REPLAY_SCRIPT=Join-Path $repo 'house-auto-joint-live.json'
$env:MICROLOGY_REPLAY_CAPTURE='1'
$env:MICROLOGY_CAPTURE_COMPACT='1'
$exe='C:\Users\behem\Projects\Micrology\target-msvc\release\sandbox.exe'
$out=Join-Path $repo 'autoj-live.stdout.log'
$err=Join-Path $repo 'autoj-live.stderr.log'
Remove-Item $out,$err -Force -ErrorAction SilentlyContinue
$proc=Start-Process -FilePath $exe -WorkingDirectory $repo -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
Write-Host "SANDBOX_PID=$($proc.Id)"
$final=Join-Path $diag 'autoj_after_120-tick000120.json'
$deadline=(Get-Date).AddSeconds(180)
while(-not (Test-Path $final) -and (Get-Date)-lt $deadline){
 Start-Sleep -Seconds 1
 $proc.Refresh()
 if($proc.HasExited){Write-Host "EARLY_EXIT=$($proc.ExitCode)";break}
}
Write-Host ("FINAL_DUMP="+$(if(Test-Path $final){'YES'}else{'NO'}))
$proc.Refresh()
if(-not $proc.HasExited){$null=$proc.CloseMainWindow();Start-Sleep -Seconds 2;$proc.Refresh();if(-not $proc.HasExited){Stop-Process -Id $proc.Id -Force}}
if(Test-Path $err){Get-Content $err -Tail 80}
if(Test-Path $final){
 $d=Get-Content $final -Raw|ConvertFrom-Json
 $sizes=@($d.fragment_dynamics|ForEach-Object{[int64]$_.cells}|Sort-Object)
 Write-Host ("STRUCTURAL="+($d.structural|ConvertTo-Json -Compress))
 Write-Host ("FRACTURE="+($d.fracture|ConvertTo-Json -Compress))
 Write-Host ("CONTACTS="+($d.contacts|ConvertTo-Json -Compress))
 Write-Host ("BACKGROUND="+($d.background|ConvertTo-Json -Compress))
 Write-Host ("PIECES="+$sizes.Count+" LARGEST="+$sizes[-1]+" TOP20="+(($sizes|Select-Object -Last 20|Sort-Object -Descending)-join ','))
}

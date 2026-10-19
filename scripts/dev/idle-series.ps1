param([string]$Exe, [string]$Out, [switch]$Integrated, [int]$AutosaveWindow = 75)
# M1 exit-criterion run (M1-SPEC §9 "App idles at 0%"): idle after startup, an edit, an undo, a
# wall fade, a save and with a dirty document (autosave armed); fade timings and frame gaps.
# Window-pixel coordinates assume the default 1456x939 window.
. "$PSScriptRoot\lab.ps1"
$idle = "$PSScriptRoot\idle.ps1"; $frames = "$PSScriptRoot\frames.py"
if ($Integrated) { $env:WGPU_POWER_PREF = 'low' } else { Remove-Item Env:WGPU_POWER_PREF -ErrorAction SilentlyContinue }
$p = Start-Lab -Exe $Exe -Dir $Out -Trace -Wait 20
$id = $p.Id; $log = (Get-ChildItem "$Out\stderr-*.txt" | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
function Now { (Get-Date).ToUniversalTime().ToString('HH:mm:ss.fff') }
function Measure-Idle([string]$What) { "$What`: " + (& $idle -ProcessId $id -Seconds 12) | Tee-Object -Append "$Out\idle.txt" }
function Frames([string]$What, [string]$From, [string]$To) { "$What (from $From UTC):"; python $frames $log $From $To }

"pid $id, $(Get-Date -f s)" | Tee-Object "$Out\idle.txt"
Measure-Idle 'startup (nothing posted yet)'
Shot $id "$Out\1-startup.png"
Post-Mouse $id hover 800 400; Start-Sleep 4   # the first frames wait for a window message (see T7 report)
Shot $id "$Out\1-startup-woken.png"
Measure-Idle 'startup, after a mouse move woke the viewport'

Click $id 62 470; Start-Sleep 1
$t = Now; Set-Field $id 1360 559 '100'; Start-Sleep 3
Frames 'edit: Table A X 0 -> 100' $t (Now) | Tee-Object -Append "$Out\frames.txt"
"title: $(Get-Title $id)" | Tee-Object -Append "$Out\idle.txt"
Measure-Idle 'after an edit (dirty: autosave armed)'

$t = Now; Click $id 523 892; Start-Sleep 3
Frames 'undo (toolbar)' $t (Now) | Tee-Object -Append "$Out\frames.txt"
"title: $(Get-Title $id)" | Tee-Object -Append "$Out\idle.txt"
Measure-Idle 'after undo (clean again)'

Click $id 60 442; Start-Sleep 1; Click $id 1321 884; Start-Sleep 1   # Room, Show walls: Always
for ($i = 0; $i -lt 8; $i++) { Post-Mouse $id wheel 720 500 -Wheel -120 }   # eye outside the room
Start-Sleep 3; Shot $id "$Out\2-walls-always.png"
foreach ($n in 1..3) {
  $t = Now; Click $id 1257 884; Start-Sleep 3   # Auto: the wall in front fades out
  Frames "fade out $n (Always -> Auto)" $t (Now) | Tee-Object -Append "$Out\frames.txt"
  if ($n -eq 1) { Shot $id "$Out\3-walls-auto.png"; Measure-Idle 'after the first wall fade' }
  if ($n -lt 3) { $t = Now; Click $id 1321 884; Start-Sleep 3; Frames "fade in $n (Auto -> Always)" $t (Now) | Tee-Object -Append "$Out\frames.txt" }
}

Click $id 700 300; Start-Sleep 1
New-Item -ItemType Directory -Force "$Out\files" | Out-Null
Click $id 150 75; Click $id 75 246   # project menu > Save As...
$d = Wait-Dialog $id; Submit-Dialog $d "$Out\files\saved.labsetup"; Start-Sleep 5
"title after Save As: $(Get-Title $id)" | Tee-Object -Append "$Out\idle.txt"
Measure-Idle 'after a save (toast gone)'

Click $id 62 470; Start-Sleep 1; Set-Field $id 1360 559 '50'; Start-Sleep 1; Click $id 700 300
$t0 = Get-Date
"title: $(Get-Title $id); dirty idle window of $AutosaveWindow s starts $(Now) UTC" | Tee-Object -Append "$Out\idle.txt"
"dirty, autosave armed: " + (& $idle -ProcessId $id -Seconds $AutosaveWindow) | Tee-Object -Append "$Out\idle.txt"
Get-ChildItem -Recurse "$Out\autosave" -File | ForEach-Object { "autosave file $($_.Name) written +{0:N1} s after the edit" -f ($_.LastWriteTime - $t0).TotalSeconds } | Tee-Object -Append "$Out\idle.txt"
Stop-Process -Id $id

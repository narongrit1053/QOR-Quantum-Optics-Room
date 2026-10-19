param([string]$Exe, [string]$Out)
# M1 end-to-end run (M1-SPEC §9 "End-to-end") on a private copy of the app, driven by posted input.
# Prints PASS/FAIL per check and exits 1 on any failure. Window-pixel coordinates assume the
# default 1456x939 window and the default demo layout.
. "$PSScriptRoot\lab.ps1"
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
$thai = Join-Path $Out ([string]::new([char[]](0x0E17, 0x0E14, 0x0E2A, 0x0E2D, 0x0E1A)))   # "ทดสอบ"
Remove-Item -Recurse -Force $thai, "$Out\autosave", "$Out\x" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $thai, "$Out\x" | Out-Null
$failed = 0
function Check([string]$What, [bool]$Ok) { "{0}  {1}" -f ($(if ($Ok) { 'PASS' } else { 'FAIL' })), $What; if (-not $Ok) { $script:failed++ } }
function Title { Get-Title $script:id }
function Menu([int]$Y, [int]$Shift = 0) { Click $id 150 75; Click $id 80 ($Y + $Shift) }   # project menu item
function JsonOf([string]$Zip) { Copy-Item -LiteralPath $Zip "$Out\x\t.zip" -Force; Expand-Archive -LiteralPath "$Out\x\t.zip" -DestinationPath "$Out\x\t" -Force; Get-Content -Raw "$Out\x\t\project.json" }
function Start-App([string]$Arg = '') { $p = Start-Lab -Exe $Exe -Dir $Out -Arg $Arg -Wait 8; $script:id = $p.Id; Post-Mouse $id hover 800 400; Start-Sleep 2 }
$NEW = 125; $OPEN = 153; $SAVE = 218; $SAVE_AS = 246; $SAVE_FOLDER = 274

Start-App
Menu $NEW; Start-Sleep 2
Check "New gives an untitled, clean document ($(Title))" ((Title) -eq 'Untitled — LabSetup')

# MB6060/M breadboard on Table A
Click $id 62 470; Start-Sleep 1; Click $id 266 409; Start-Sleep 1
Post-Mouse $id wheel 390 700 -Wheel -240; Post-Mouse $id wheel 390 700 -Wheel -240; Start-Sleep 1
Click $id 312 760; Start-Sleep 2; Shot $id "$Out\01-breadboard.png"
Check "Breadboard added (scene row, dirty title)" ((Title) -like '*Edited*')

# Custom shape, corner 2 X 300 -> 350, add a corner, add a circle zone
Click $id 1384 242; Start-Sleep 2
Set-Field $id 1212 363 '350'; Start-Sleep 1
Click $id 1202 459; Start-Sleep 2; Send-Key $id 0x1B; Start-Sleep 1
for ($i = 0; $i -lt 6; $i++) { Post-Mouse $id wheel 1290 600 -Wheel -120 }; Start-Sleep 1
Click $id 1288 909; Start-Sleep 2; Shot $id "$Out\02-custom-zone.png"

# Room: add a door, move it near corner 3, make the room an L (the door is refit, toast)
Click $id 60 442; Start-Sleep 2
Click $id 1185 800; Start-Sleep 2
Set-Field $id 1240 867 '300'; Start-Sleep 1
Click $id 1204 194; Start-Sleep 0.6; Shot $id "$Out\03-l-shape-toast.png"

# Undo x3, redo x2 (toolbar)
foreach ($i in 1..3) { Click $id 523 892; Start-Sleep 1 }
foreach ($i in 1..2) { Click $id 555 892; Start-Sleep 1 }
Start-Sleep 2; Shot $id "$Out\04-undo3-redo2.png"

# Save As (archive) into a folder with a Thai name
Menu $SAVE_AS; Submit-Dialog (Wait-Dialog $id) "$thai\setup.labsetup"; Start-Sleep 3
Check "Save As wrote the archive and cleaned the title ($(Title))" ((Test-Path -LiteralPath "$thai\setup.labsetup") -and (Title) -eq 'setup — LabSetup')
$saved = JsonOf "$thai\setup.labsetup"
$doc = $saved | ConvertFrom-Json
$bb = $doc.platforms | Where-Object name -eq 'Breadboard 1'
Check "Saved breadboard: custom polygon with corner 2 at x=350, 5 corners, a circle zone" (
  $bb.outline.Polygon.corners.Count -eq 5 -and $bb.outline.Polygon.corners[1].point[0] -eq 350 -and $bb.hole_grid.exclusions[0].shape.Circle -ne $null)
Check "Saved room: rectangle with Door 1 and Door 2 (wall 3, offset 300) after undo x3 / redo x2" (
  $doc.room.outline.Count -eq 4 -and $doc.room.openings.Count -eq 2 -and ($doc.room.openings | Where-Object name -eq 'Door 2').offset -eq 300)

# Close, reopen: identical and clean
Close-Main $id; for ($i = 0; $i -lt 60 -and (Get-Process -Id $id -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 500 }
Check "Closing a clean document quits" (-not (Get-Process -Id $id -ErrorAction SilentlyContinue))
Start-App "$thai\setup.labsetup"
Check "Reopened title is clean ($(Title))" ((Title) -eq 'setup — LabSetup')
Menu $SAVE_FOLDER; Submit-Dialog (Wait-Dialog $id) "$thai\setup-folder.labsetup"; Start-Sleep 3
$folderJson = Get-Content -Raw -LiteralPath "$thai\setup-folder\project.json"
Check "Reopened content is identical (Save as Folder project.json == archive project.json)" ($folderJson -eq $saved)

# Open project.json, edit, autosave, kill, restart, Restore
Menu $OPEN; Submit-Dialog (Wait-Dialog $id) "$thai\setup-folder\project.json"; Start-Sleep 3
Click $id 62 470; Start-Sleep 1; Set-Field $id 1360 559 '250'; Start-Sleep 1; Set-Field $id 1360 595 '-100'
Check "Edits after opening project.json mark the title ($(Title))" ((Title) -like 'setup-folder — Edited*')
Start-Sleep 68
$auto = Get-ChildItem -Recurse "$Out\autosave" -Filter autosave.labsetup | Select-Object -First 1
Check "Autosave written within ~60 s" ($auto -ne $null)
$autoJson = JsonOf $auto.FullName
Stop-Process -Id $id -Force; Start-Sleep 2
Start-App
Start-Sleep 2; Shot $id "$Out\05-recovery-sheet.png"   # let the sheet finish sliding in
Click $id 742 483; Start-Sleep 3   # Restore
Check "Restore brings the document back, still dirty ($(Title))" ((Title) -eq 'setup-folder — Edited — LabSetup')
Menu $SAVE 17; Start-Sleep 3   # the "Recovered — not saved yet" note moves the menu down
Check "Restored content == autosaved content (after Save)" ((Get-Content -Raw -LiteralPath "$thai\setup-folder\project.json") -eq $autoJson)

# Inch display + Enter on unchanged fields leaves the title clean
Click $id 958 58; Start-Sleep 1; Click $id 62 470; Start-Sleep 1
foreach ($y in 559, 278, 703) { Click $id 1360 $y; Send-Key $id 0x0D; Start-Sleep 1 }
Shot $id "$Out\06-inch.png"
Check "Inch display, Enter on unchanged X / Width / Thickness keeps the title clean ($(Title))" ((Title) -eq 'setup-folder — LabSetup')
Click $id 920 58; Start-Sleep 1   # back to mm (a preference saved in settings.json)
Close-Main $id

"$failed check(s) failed"
if ($failed) { exit 1 }

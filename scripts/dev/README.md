# Dev scripts for checking the running app

These touch only the LabSetup window whose process id you pass. They never move the real cursor or focus other windows.

| Script | Use |
|---|---|
| `shot.ps1 -ProcessId <pid> -Out <file.png>` | Capture the window (PrintWindow, works even when covered) |
| `post.ps1 -ProcessId <pid> -Action click\|hover\|drag\|wheel -X <x> -Y <y> [-X2 -Y2] [-Wheel 120]` | Post mouse messages; coordinates are window pixels as seen in a screenshot |
| `idle.ps1 -ProcessId <pid> [-Seconds 10]` | CPU and GPU use of an idle process (all GPU engines) |
| `lab.ps1` (dot-source) | The same as functions (`Start-Lab`, `Shot`, `Post-Mouse`, `Click`, `Send-Key`, `Send-Text`, `Set-Field`, `Get-Title`, `Wait-Dialog`, `Submit-Dialog`, `Close-Main`) for longer runs |
| `frames.py <log> <from HH:MM:SS.fff> [<to>]` | Frames rendered after a moment (UTC, from a `RUST_LOG=lab_viewport=debug` log): count, last frame, gaps, scales |
| `idle-series.ps1 -Exe <copy> -Out <dir> [-Integrated]` | M1 §9 idle run: idle after startup, edit, undo, wall fade, save and with autosave armed, plus fade timings |
| `e2e.ps1 -Exe <copy> -Out <dir>` | M1 §9 end-to-end run; prints PASS/FAIL per check, exits 1 on a failure |

Start the app with `Start-Process ... -PassThru` (or `Start-Lab`) and keep its `Id`; several copies may run at once, so never act on processes you did not start. `RUST_LOG=lab_viewport=debug` logs every rendered frame; `LABSETUP_TRACE=1` logs frame pacing. `WGPU_POWER_PREF=low` selects the integrated GPU.

Notes from T7:
- The app window is the visible top-level window of class `Window Class`; `Process.MainWindowHandle` may be winit's hidden window.
- winit derives text from `WM_KEYDOWN`, so `Send-Text` posts key presses (numbers on the numpad, which ignores the keyboard layout); a bare `WM_CHAR` is ignored.
- Posted Ctrl/Shift shortcuts are unreliable: the app's next real input message resyncs the key state. Use the project menu and the toolbar instead.
- Right after startup the viewport may stay blank until a window message arrives; `Start-Lab` callers post a hover first.
- `e2e.ps1` and `idle-series.ps1` assume the default 1456 × 939 window.

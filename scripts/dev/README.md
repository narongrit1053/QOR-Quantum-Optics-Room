# Dev scripts for checking the running app

These touch only the LabSetup window whose process id you pass. They never move the real cursor or focus other windows.

| Script | Use |
|---|---|
| `shot.ps1 -ProcessId <pid> -Out <file.png>` | Capture the window (PrintWindow, works even when covered) |
| `post.ps1 -ProcessId <pid> -Action click\|hover\|drag\|wheel -X <x> -Y <y> [-X2 -Y2] [-Wheel 120]` | Post mouse messages; coordinates are window pixels as seen in a screenshot |
| `idle.ps1 -ProcessId <pid> [-Seconds 10]` | CPU and GPU use of an idle process (all GPU engines) |

Start the app with `Start-Process ... -PassThru` and keep its `Id`; several copies may run at once, so never act on processes you did not start. `RUST_LOG=lab_viewport=debug` logs every rendered frame; `LABSETUP_TRACE=1` logs frame pacing.

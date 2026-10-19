param([int]$ProcessId, [string]$Action, [int]$X, [int]$Y, [int]$X2 = 0, [int]$Y2 = 0, [int]$Wheel = 0)
# Posts synthetic mouse messages to the app window only. Coordinates are in screenshot
# (window) pixels; the real cursor is never moved.
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class P {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[P]::SetProcessDPIAware() | Out-Null
. "$PSScriptRoot\lab.ps1"; $h = Find-AppWindow $ProcessId   # MainWindowHandle may be winit's hidden window
$r = New-Object P+RECT; [P]::GetWindowRect($h, [ref]$r) | Out-Null
$o = New-Object P+POINT; [P]::ClientToScreen($h, [ref]$o) | Out-Null
$dx = $o.X - $r.L; $dy = $o.Y - $r.T
function L([int]$x, [int]$y) { [IntPtr](((($y - $dy) -band 0xFFFF) -shl 16) -bor (($x - $dx) -band 0xFFFF)) }
function Send([uint32]$m, [int]$w, [IntPtr]$l) { [P]::PostMessage($h, $m, [IntPtr]$w, $l) | Out-Null; Start-Sleep -Milliseconds 30 }
$MOVE = 0x200; $LDOWN = 0x201; $LUP = 0x202; $WHEEL = 0x20A
switch ($Action) {
  'click' { Send $MOVE 0 (L $X $Y); Send $LDOWN 1 (L $X $Y); Send $LUP 0 (L $X $Y) }
  'hover' { Send $MOVE 0 (L $X $Y) }
  'drag'  {
    Send $MOVE 0 (L $X $Y); Send $LDOWN 1 (L $X $Y)
    for ($i = 1; $i -le 12; $i++) { Send $MOVE 1 (L ([int]($X + ($X2 - $X) * $i / 12)) ([int]($Y + ($Y2 - $Y) * $i / 12))) }
    Send $LUP 0 (L $X2 $Y2)
  }
  'wheel' {
    Send $MOVE 0 (L $X $Y)
    $screen = [IntPtr]((($r.T + $Y) -shl 16) -bor (($r.L + $X) -band 0xFFFF))
    Send $WHEEL ($Wheel -shl 16) $screen
  }
}
"$Action posted"

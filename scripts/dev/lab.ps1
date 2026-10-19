# Helpers for driving one LabSetup process (dot-source: `. scripts/dev/lab.ps1`).
# Everything is posted to the window of the process id you pass; the real cursor, the real
# keyboard and other windows are never touched.
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Lab {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll")] public static extern bool GetKeyboardState(byte[] s);
  [DllImport("user32.dll")] public static extern bool SetKeyboardState(byte[] s);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
  public static string Txt(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
}
"@
[Lab]::SetProcessDPIAware() | Out-Null

# The app's own top-level window (class "Window Class"), not winit's hidden event target,
# which is what Process.MainWindowHandle may return.
function Find-AppWindow([int]$ProcessId) {
  $found = New-Object System.Collections.ArrayList
  [Lab]::EnumWindows({ param($h, $l) $p = 0; [Lab]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
    if ($p -eq $ProcessId -and [Lab]::Cls($h) -eq 'Window Class' -and [Lab]::IsWindowVisible($h)) { [void]$found.Add($h) }; $true }, [IntPtr]::Zero) | Out-Null
  if (-not $found.Count) { throw "no app window for pid $ProcessId" }
  [IntPtr]$found[0]
}

# Starts a copy of the app with a private autosave dir and settings file; returns the process.
function Start-Lab([string]$Exe, [string]$Dir, [string]$Arg = '', [int]$Wait = 6, [switch]$Trace) {
  New-Item -ItemType Directory -Force $Dir | Out-Null
  $env:LABSETUP_AUTOSAVE_DIR = "$Dir\autosave"; $env:LABSETUP_SETTINGS = "$Dir\settings.json"
  if ($Trace) { $env:LABSETUP_TRACE = '1'; $env:RUST_LOG = 'lab_viewport=debug' } else { Remove-Item Env:LABSETUP_TRACE, Env:RUST_LOG -ErrorAction SilentlyContinue }
  # Exe copies need DXC beside them too, or shaders compile with the slow FXC.
  $dxc = Join-Path (Split-Path $Exe) 'dxcompiler.dll'
  if (-not (Test-Path $dxc)) { Copy-Item "$PSScriptRoot/../../third_party/dxc/dxcompiler.dll" $dxc }
  $a = @{ FilePath = $Exe; PassThru = $true; RedirectStandardError = "$Dir\stderr-$(Get-Date -f HHmmss).txt" }
  if ($Arg) { $a.ArgumentList = "`"$Arg`"" }
  $p = Start-Process @a; Start-Sleep $Wait
  for ($i = 0; $i -lt 60; $i++) { try { Find-AppWindow $p.Id | Out-Null; break } catch { if ($p.HasExited) { throw "LabSetup exited with code $($p.ExitCode)" }; Start-Sleep -Milliseconds 500 } }
  $p
}

function Get-Title([int]$ProcessId) { [Lab]::Txt((Find-AppWindow $ProcessId)) }

function Shot([int]$ProcessId, [string]$Out) {
  $h = Find-AppWindow $ProcessId
  $r = New-Object Lab+RECT; [Lab]::GetWindowRect($h, [ref]$r) | Out-Null
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
  [Lab]::PrintWindow($h, $hdc, 2) | Out-Null   # PW_RENDERFULLCONTENT: works while covered
  $g.ReleaseHdc($hdc); $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()
}

# Mouse input in screenshot (window) pixels.
function Post-Mouse([int]$ProcessId, [string]$Action, [int]$X, [int]$Y, [int]$X2 = 0, [int]$Y2 = 0, [int]$Wheel = 0, [int]$Buttons = 1) {
  $h = Find-AppWindow $ProcessId
  $r = New-Object Lab+RECT; [Lab]::GetWindowRect($h, [ref]$r) | Out-Null
  $o = New-Object Lab+POINT; [Lab]::ClientToScreen($h, [ref]$o) | Out-Null
  $dx = $o.X - $r.L; $dy = $o.Y - $r.T
  $L = { param([int]$x, [int]$y) [IntPtr](((($y - $dy) -band 0xFFFF) -shl 16) -bor (($x - $dx) -band 0xFFFF)) }
  $S = { param([uint32]$m, [int]$w, [IntPtr]$l) [Lab]::PostMessage($h, $m, [IntPtr]$w, $l) | Out-Null; Start-Sleep -Milliseconds 30 }
  switch ($Action) {
    'click'  { & $S 0x200 0 (& $L $X $Y); & $S 0x201 1 (& $L $X $Y); & $S 0x202 0 (& $L $X $Y) }
    'dclick' { & $S 0x200 0 (& $L $X $Y); & $S 0x201 1 (& $L $X $Y); & $S 0x202 0 (& $L $X $Y); & $S 0x203 1 (& $L $X $Y); & $S 0x202 0 (& $L $X $Y) }
    'hover'  { & $S 0x200 0 (& $L $X $Y) }
    'drag'   {
      & $S 0x200 0 (& $L $X $Y); & $S 0x201 $Buttons (& $L $X $Y)
      for ($i = 1; $i -le 12; $i++) { & $S 0x200 $Buttons (& $L ([int]($X + ($X2 - $X) * $i / 12)) ([int]($Y + ($Y2 - $Y) * $i / 12))) }
      & $S 0x202 0 (& $L $X2 $Y2)
    }
    'wheel'  { & $S 0x200 0 (& $L $X $Y); & $S 0x20A ($Wheel -shl 16) ([IntPtr]((($r.T + $Y) -shl 16) -bor (($r.L + $X) -band 0xFFFF))) }
  }
  Start-Sleep -Milliseconds 300
}
function Click([int]$ProcessId, [int]$X, [int]$Y) { Post-Mouse $ProcessId click $X $Y }

# A key press (virtual-key code). Unreliable with -Ctrl/-Shift: the app's next real input
# message resyncs the key state, so prefer menus and buttons for shortcuts. Ctrl/Shift: winit reads modifiers from the keyboard state,
# which posted messages do not change, so this attaches to the app's input queue, sets the shared
# state and also posts the modifier key itself while the key is processed.
function Send-Key([int]$ProcessId, [int]$Vk, [switch]$Ctrl, [switch]$Shift) {
  $h = Find-AppWindow $ProcessId
  $pid0 = 0; $tid = [Lab]::GetWindowThreadProcessId($h, [ref]$pid0); $me = [Lab]::GetCurrentThreadId()
  $mods = @(); if ($Ctrl) { $mods += ,@(0x11, 0xA2, 0x1D) }; if ($Shift) { $mods += ,@(0x10, 0xA0, 0x2A) }
  $down = { param($vk, $sc) [Lab]::PostMessage($h, 0x0100, [IntPtr]$vk, [IntPtr](($sc -shl 16) -bor 1)) | Out-Null; Start-Sleep -Milliseconds 50 }
  $up = { param($vk, $sc) [Lab]::PostMessage($h, 0x0101, [IntPtr]$vk, [IntPtr]([int64]0xC0000001 -bor ($sc -shl 16))) | Out-Null; Start-Sleep -Milliseconds 50 }
  if ($mods.Count) {
    [Lab]::AttachThreadInput($me, $tid, $true) | Out-Null
    $ks = New-Object byte[] 256; [Lab]::GetKeyboardState($ks) | Out-Null
    foreach ($m in $mods) { $ks[$m[0]] = 0x80; $ks[$m[1]] = 0x80 }
    [Lab]::SetKeyboardState($ks) | Out-Null
    foreach ($m in $mods) { & $down $m[0] $m[2] }
  }
  & $down $Vk 0; & $up $Vk 0
  if ($mods.Count) {
    foreach ($m in $mods) { & $up $m[0] $m[2] }
    Start-Sleep -Milliseconds 250
    foreach ($m in $mods) { $ks[$m[0]] = 0; $ks[$m[1]] = 0 }
    [Lab]::SetKeyboardState($ks) | Out-Null
    [Lab]::AttachThreadInput($me, $tid, $false) | Out-Null
  }
  Start-Sleep -Milliseconds 150
}
# Types digits, letters, '.', '-' and spaces into the focused field as key presses (winit
# derives the text from WM_KEYDOWN; a bare WM_CHAR is ignored).
function Send-Text([int]$ProcessId, [string]$Text) {
  foreach ($c in $Text.ToUpper().ToCharArray()) {
    # Numpad keys for numbers: the main-row keys follow the thread's keyboard layout (Thai here).
    $vk = switch -regex ($c) { '\d' { 0x60 + [int][string]$c } '\.' { 0x6E } '-' { 0x6D } ' ' { 0x20 } default { [int]$c } }
    Send-Key $ProcessId $vk
  }
}
# Replaces a field's text (End, then Backspace) and commits it with Enter.
function Set-Field([int]$ProcessId, [int]$X, [int]$Y, [string]$Text) {
  Click $ProcessId $X $Y; Send-Key $ProcessId 0x23
  for ($i = 0; $i -lt 14; $i++) { Send-Key $ProcessId 0x08 }
  Send-Text $ProcessId $Text; Send-Key $ProcessId 0x0D
}
function Close-Main([int]$ProcessId) { [Lab]::PostMessage((Find-AppWindow $ProcessId), 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }

# Win32 common file dialogs owned by the process.
function Wait-Dialog([int]$ProcessId, [int]$Seconds = 10) {
  for ($i = 0; $i -lt $Seconds * 5; $i++) {
    $found = New-Object System.Collections.ArrayList
    [Lab]::EnumWindows({ param($h, $l) $p = 0; [Lab]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
      if ($p -eq $ProcessId -and [Lab]::Cls($h) -eq '#32770' -and [Lab]::IsWindowVisible($h)) { [void]$found.Add($h) }; $true }, [IntPtr]::Zero) | Out-Null
    if ($found.Count) { return [IntPtr]$found[0] }
    Start-Sleep -Milliseconds 200
  }
  throw "no dialog"
}
# Types a path into the dialog's file name box and presses OK.
function Submit-Dialog([IntPtr]$Dialog, [string]$Path) {
  $edits = New-Object System.Collections.ArrayList
  [Lab]::EnumChildWindows($Dialog, { param($h, $l) if ([Lab]::Cls($h) -eq 'Edit' -and [Lab]::IsWindowVisible($h)) { [void]$edits.Add($h) }; $true }, [IntPtr]::Zero) | Out-Null
  [Lab]::SendMessage([IntPtr]$edits[0], 0x000C, [IntPtr]::Zero, $Path) | Out-Null   # WM_SETTEXT
  Start-Sleep -Milliseconds 200
  [Lab]::PostMessage($Dialog, 0x0111, [IntPtr]1, [IntPtr]::Zero) | Out-Null          # IDOK
  Start-Sleep -Milliseconds 800
}

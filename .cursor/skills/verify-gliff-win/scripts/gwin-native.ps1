<#
.SYNOPSIS
  Drive the real gliff-windows.exe on this Windows machine against the
  loopback server, and keep the evidence. Verification scaffolding only.

.DESCRIPTION
  The loopback is gwin-fake-ssh.exe (passed to the client with --ssh) running
  gwin-fake-server.exe on this machine: no network, no sshd, no keys, no host
  trust. The fake server writes what it received to input.jsonl in the run
  folder (GWIN_FAKE_INPUT_LOG), which is the remote half of the evidence.

  Each verb prints one JSON line and exits 1 when "ok" is false.

    launch  -Bundle DIR [-Fault NAME] [-Clipboard]   start an owned client
    doctor  -Run DIR                                 is it worth driving?
    shot    -Run DIR -Name NAME                      PNG of the owned window
    type    -Run DIR -Text TEXT                      click, type, give back
    resize  -Run DIR -Width W -Height H              resize the owned window
    drop    -Run DIR                                 kill the owned server
    cleanup -Run DIR                                 end what launch started

  The bundle folder holds gliff-windows.exe, gwin-fake-ssh.exe and
  gwin-fake-server.exe from one build (`test-hooks` is not needed).
  Windows PowerShell 5.1 or later; nothing is installed.

  Rules: the client always gets the run folder's own config and log
  directories, so it never shares the single-instance lock, settings or
  recent hosts of an installed client. Cleanup ends only the PIDs this run
  started, after checking their paths; never by name. Evidence (run.json,
  input.jsonl, stats.json, logs\, evidence\) survives cleanup.
#>
param(
  [Parameter(Mandatory = $true, Position = 0)]
  [ValidateSet('launch', 'doctor', 'shot', 'type', 'resize', 'drop', 'cleanup')]
  [string]$Verb,
  [string]$Bundle,
  [string]$Run,
  [string]$Fault = '',
  [switch]$Clipboard,
  [string]$Name = 'shot',
  [string]$Text = 'gliff',
  [int]$Width = 1024,
  [int]$Height = 640,
  [int]$TimeoutSec = 30
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

# Windows PowerShell 5.1 has System.Drawing; PowerShell 7 splits it, and
# from .NET 9 also needs System.Private.Windows.Core.
$drawing = @('System.Drawing')
if ($PSVersionTable.PSEdition -eq 'Core') {
  $drawing = @('System.Drawing.Common', 'System.Drawing.Primitives', 'System.Runtime.InteropServices')
  if ([Environment]::Version.Major -ge 9) { $drawing += 'System.Private.Windows.Core' }
}
$GwinNativeSource = @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public static class GwinNative {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)]
  public struct POINT { public int X, Y; }

  [StructLayout(LayoutKind.Sequential)]
  struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)]
  struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Explicit)]
  struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)]
  struct INPUT { public uint type; public INPUTUNION u; }

  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, INPUT[] inputs, int size);

  public static void DpiAware() { SetProcessDPIAware(); }
  public static bool Visible(IntPtr h) { return IsWindowVisible(h); }
  public static bool IsForeground(IntPtr h) { return GetForegroundWindow() == h; }
  public static bool Focus(IntPtr h) { return SetForegroundWindow(h); }

  public static int[] Rect(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    return new int[] { r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top };
  }

  public static int[] ClientCentre(IntPtr h) {
    RECT r; GetClientRect(h, out r);
    POINT p; p.X = (r.Right - r.Left) / 2; p.Y = (r.Bottom - r.Top) / 2;
    ClientToScreen(h, ref p);
    return new int[] { p.X, p.Y };
  }

  public static bool SetSize(IntPtr h, int w, int ht) {
    // SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
    return SetWindowPos(h, IntPtr.Zero, 0, 0, w, ht, 0x0002 | 0x0004 | 0x0010);
  }

  static void Send(INPUT[] inputs) {
    uint sent = SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
    if (sent != inputs.Length) throw new Exception("SendInput refused: " + Marshal.GetLastWin32Error());
  }

  // A left click at a screen point (MOVE | ABSOLUTE | VIRTUALDESK, then down and up).
  public static void Click(int x, int y) {
    int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77);
    int vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
    INPUT[] i = new INPUT[3];
    for (int k = 0; k < 3; k++) i[k].type = 0;
    i[0].u.mi.dx = (int)(((x - vx) * 65535L) / Math.Max(1, vw - 1));
    i[0].u.mi.dy = (int)(((y - vy) * 65535L) / Math.Max(1, vh - 1));
    i[0].u.mi.dwFlags = 0x0001 | 0x8000 | 0x4000;
    i[1].u.mi.dwFlags = 0x0002;
    i[2].u.mi.dwFlags = 0x0004;
    Send(i);
  }

  // One key by scan code (KEYEVENTF_SCANCODE, plus EXTENDEDKEY when e0).
  public static void Key(ushort scan, bool e0, bool down) {
    INPUT[] i = new INPUT[1];
    i[0].type = 1;
    i[0].u.ki.wScan = scan;
    i[0].u.ki.dwFlags = 0x0008 | (e0 ? 0x0001u : 0u) | (down ? 0u : 0x0002u);
    Send(i);
  }

  public static void Png(int x, int y, int w, int h, string path) {
    using (Bitmap b = new Bitmap(w, h, PixelFormat.Format32bppArgb)) {
      using (Graphics g = Graphics.FromImage(b)) {
        g.CopyFromScreen(x, y, 0, 0, new Size(w, h));
      }
      b.Save(path, ImageFormat.Png);
    }
  }
}
'@
Add-Type -ReferencedAssemblies $drawing -TypeDefinition $GwinNativeSource
[GwinNative]::DpiAware()

# Scan codes (set 1) for the characters `type` sends, and the evdev code the
# remote machine must receive for each (evdev equals the set-1 code here).
$ScanOf = @{}
$i = 0
foreach ($c in 'qwertyuiop'.ToCharArray()) { $ScanOf[[string]$c] = 0x10 + $i; $i++ }
$i = 0
foreach ($c in 'asdfghjkl'.ToCharArray()) { $ScanOf[[string]$c] = 0x1e + $i; $i++ }
$i = 0
foreach ($c in 'zxcvbnm'.ToCharArray()) { $ScanOf[[string]$c] = 0x2c + $i; $i++ }
$i = 0
foreach ($c in '1234567890'.ToCharArray()) { $ScanOf[[string]$c] = 0x02 + $i; $i++ }
$ScanOf[' '] = 0x39

function Out-Line($fields) {
  $fields | ConvertTo-Json -Compress -Depth 6
  if (-not $fields.ok) { exit 1 }
}

function Read-RunState {
  if (-not $Run) { throw '-Run DIR is required' }
  $file = Join-Path $Run 'run.json'
  if (-not (Test-Path $file)) { throw "no run.json in $Run" }
  Get-Content $file -Raw | ConvertFrom-Json
}

function Save-RunState($state) {
  $state | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $state.run 'run.json')
}

# The owned client, or $null when it has exited or the PID is someone else's.
function Get-OwnedClient($state) {
  $p = Get-Process -Id $state.pid -ErrorAction SilentlyContinue
  if (-not $p) { return $null }
  try { $path = $p.Path } catch { return $null }
  if ($path -ne $state.exe) { return $null }
  $p
}

# PIDs below the owned client whose program is inside the bundle: the fake
# ssh and the fake server it runs.
function Get-OwnedChildren($state) {
  $all = @(Get-CimInstance Win32_Process)
  $tree = @($state.pid)
  $found = @()
  $grew = $true
  while ($grew) {
    $grew = $false
    foreach ($p in $all) {
      if (($tree -contains [int]$p.ParentProcessId) -and -not ($tree -contains [int]$p.ProcessId)) {
        $tree += [int]$p.ProcessId
        if ($p.ExecutablePath -and $p.ExecutablePath.StartsWith($state.bundle, [StringComparison]::OrdinalIgnoreCase)) {
          $found += [pscustomobject]@{ pid = [int]$p.ProcessId; exe = $p.ExecutablePath }
        }
        $grew = $true
      }
    }
  }
  $found
}

function Read-InputLog($state) {
  if (-not (Test-Path $state.input_log)) { return @() }
  @(Get-Content $state.input_log | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
}

function Read-Stats($state) {
  for ($try = 0; $try -lt 5; $try++) {
    try {
      if (Test-Path $state.stats) { return (Get-Content $state.stats -Raw | ConvertFrom-Json) }
      return $null
    } catch { Start-Sleep -Milliseconds 100 }
  }
  $null
}

function Get-Streaming($stats) {
  if (-not $stats -or -not ($stats.PSObject.Properties.Name -contains 'sessions')) { return $null }
  @($stats.sessions | Where-Object { $_.status -eq 'streaming' }) | Select-Object -First 1
}

function Test-Doctor($state) {
  $reasons = @()
  $client = Get-OwnedClient $state
  if (-not $client) { $reasons += 'client_not_running_or_not_owned' }
  $hash = (Get-FileHash $state.exe -Algorithm SHA256).Hash.ToLower()
  if ($hash -ne $state.exe_sha256) { $reasons += 'binary_changed' }
  $hwnd = 0
  if ($client) {
    $client.Refresh()
    $hwnd = [int64]$client.MainWindowHandle
    if ($hwnd -eq 0 -or -not [GwinNative]::Visible([IntPtr]$hwnd)) { $reasons += 'no_visible_window' }
  }
  $first = Get-Streaming (Read-Stats $state)
  Start-Sleep -Milliseconds 1500
  $second = Get-Streaming (Read-Stats $state)
  if (-not $second) { $reasons += 'no_streaming_session' }
  elseif ($first -and $second.frames -le $first.frames) { $reasons += 'frames_not_increasing' }
  $hello = @(Read-InputLog $state | Where-Object { $_.ev -eq 'hello' }).Count
  if ($hello -lt 1) { $reasons += 'remote_saw_no_hello' }
  [ordered]@{
    step = 'doctor'; ok = ($reasons.Count -eq 0); reasons = $reasons
    pid = $state.pid; exe = $state.exe; exe_sha256 = $hash; hwnd = $hwnd
    frames = $(if ($second) { $second.frames } else { 0 })
    keys_down = $(if ($second) { $second.keys_down } else { $null })
    remote_hellos = $hello; children = @(Get-OwnedChildren $state); run = $state.run
  }
}

function Get-Hwnd($state) {
  $client = Get-OwnedClient $state
  if (-not $client) { throw 'the owned client is not running' }
  $client.Refresh()
  [IntPtr]$client.MainWindowHandle
}

function Save-Shot($state, $name) {
  $hwnd = Get-Hwnd $state
  [void][GwinNative]::Focus($hwnd)
  Start-Sleep -Milliseconds 300
  $r = [GwinNative]::Rect($hwnd)
  $dir = Join-Path $state.run 'evidence'
  New-Item -ItemType Directory -Force $dir | Out-Null
  $path = Join-Path $dir ($name + '.png')
  [GwinNative]::Png($r[0], $r[1], $r[2], $r[3], $path)
  [ordered]@{ path = $path; rect = $r; foreground = [GwinNative]::IsForeground($hwnd) }
}

switch ($Verb) {
  'launch' {
    if (-not $Bundle) { throw '-Bundle DIR is required' }
    $Bundle = (Resolve-Path $Bundle).Path
    $exe = Join-Path $Bundle 'gliff-windows.exe'
    $ssh = Join-Path $Bundle 'gwin-fake-ssh.exe'
    foreach ($f in @($exe, $ssh, (Join-Path $Bundle 'gwin-fake-server.exe'))) {
      if (-not (Test-Path $f)) { throw "missing $f" }
    }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $runDir = Join-Path $env:TEMP ('gwin-verify-' + $stamp + '-' + [guid]::NewGuid().ToString('N').Substring(0, 6))
    New-Item -ItemType Directory $runDir | Out-Null
    $state = [ordered]@{
      run = $runDir; bundle = $Bundle; exe = $exe
      exe_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
      input_log = (Join-Path $runDir 'input.jsonl'); stats = (Join-Path $runDir 'stats.json')
      config = (Join-Path $runDir 'config'); logs = (Join-Path $runDir 'logs')
      fault = $Fault; clipboard = [bool]$Clipboard; started = (Get-Date).ToString('o'); pid = 0
    }
    $argList = @(
      '--config-dir', ('"' + $state.config + '"'), '--log-dir', ('"' + $state.logs + '"'),
      '--stats-out', ('"' + $state.stats + '"'), '--ssh', ('"' + $ssh + '"'), '--super-l', 'off', '--headless'
    )
    if (-not $Clipboard) { $argList += '--no-clipboard' }
    $argList += 'verify@loopback'
    # The client passes its environment to ssh, and ssh to the server.
    $env:GWIN_FAKE_INPUT_LOG = $state.input_log
    if ($Fault) { $env:GWIN_FAKE_SERVER = $Fault }
    try {
      $p = Start-Process -FilePath $exe -ArgumentList $argList -PassThru
    } finally {
      Remove-Item Env:GWIN_FAKE_INPUT_LOG -ErrorAction SilentlyContinue
      Remove-Item Env:GWIN_FAKE_SERVER -ErrorAction SilentlyContinue
    }
    $state.pid = $p.Id
    $state = [pscustomobject]$state
    Save-RunState $state
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
      $p.Refresh()
      if ($p.HasExited) { break }
      if ([int64]$p.MainWindowHandle -ne 0 -and (Get-Streaming (Read-Stats $state))) { $ready = $true; break }
      Start-Sleep -Milliseconds 250
    }
    Out-Line ([ordered]@{
      step = 'launch'; ok = $ready; run = $runDir; pid = $state.pid; exe = $exe; exe_sha256 = $state.exe_sha256
      exited = $p.HasExited; fault = $Fault
    })
  }
  'doctor' { Out-Line (Test-Doctor (Read-RunState)) }
  'shot' {
    $state = Read-RunState
    $s = Save-Shot $state $Name
    Out-Line ([ordered]@{ step = 'shot'; ok = (Test-Path $s.path); path = $s.path; rect = $s.rect; foreground = $s.foreground })
  }
  'type' {
    $state = Read-RunState
    $chars = @($Text.ToLower().ToCharArray() | ForEach-Object { [string]$_ })
    foreach ($c in $chars) { if (-not $ScanOf.ContainsKey($c)) { throw "type sends a-z, 0-9 and space only, not '$c'" } }
    $before = @(Read-InputLog $state).Count
    $hwnd = Get-Hwnd $state
    $pre = Save-Shot $state 'type-before'
    $centre = [GwinNative]::ClientCentre($hwnd)
    # Clicking the picture gives the window the keyboard (KEYS-1).
    [GwinNative]::Click($centre[0], $centre[1])
    Start-Sleep -Milliseconds 400
    foreach ($c in $chars) {
      [GwinNative]::Key([uint16]$ScanOf[$c], $false, $true); Start-Sleep -Milliseconds 30
      [GwinNative]::Key([uint16]$ScanOf[$c], $false, $false); Start-Sleep -Milliseconds 30
    }
    Start-Sleep -Milliseconds 300
    $mid = Save-Shot $state 'type-after'
    # Give the keyboard back: the default release chord, Ctrl+Alt+Home.
    [GwinNative]::Key(0x1d, $false, $true); [GwinNative]::Key(0x38, $false, $true)
    [GwinNative]::Key(0x47, $true, $true); [GwinNative]::Key(0x47, $true, $false)
    [GwinNative]::Key(0x38, $false, $false); [GwinNative]::Key(0x1d, $false, $false)
    Start-Sleep -Milliseconds 700
    $after = @(Read-InputLog $state)
    $new = @($after | Select-Object -Skip $before)
    # The typed keys, in order, ignoring the click and the release chord's
    # modifiers (Ctrl 29, Alt 56, Home 102), which the remote may also see.
    $want = @()
    foreach ($c in $chars) { $want += ('{0}:down' -f $ScanOf[$c]); $want += ('{0}:up' -f $ScanOf[$c]) }
    $got = @($new | Where-Object { $_.ev -eq 'key' -and @(29, 56, 97, 100, 102) -notcontains [int]$_.code } |
      ForEach-Object { '{0}:{1}' -f $_.code, $(if ($_.down) { 'down' } else { 'up' }) })
    $held = @{}
    foreach ($k in @($after | Where-Object { $_.ev -eq 'key' })) {
      if ($k.down) { $held[[int]$k.code] = $true } else { $held.Remove([int]$k.code) }
    }
    $stats = Get-Streaming (Read-Stats $state)
    $reasons = @()
    if (($got -join ',') -ne ($want -join ',')) { $reasons += 'remote_keys_differ' }
    if ($held.Count -gt 0) { $reasons += 'remote_holds_keys' }
    if (-not $pre.foreground) { $reasons += 'window_not_foreground' }
    $result = [ordered]@{
      step = 'type'; ok = ($reasons.Count -eq 0); reasons = $reasons; text = $Text
      want = $want; got = $got; remote_held = @($held.Keys)
      client_keys_down = $(if ($stats) { $stats.keys_down } else { $null })
      shots = @($pre.path, $mid.path); new_remote_lines = $new.Count
    }
    $result | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $state.run 'evidence\type.json')
    Out-Line $result
  }
  'resize' {
    $state = Read-RunState
    $hwnd = Get-Hwnd $state
    $before = @(Read-InputLog $state | Where-Object { $_.ev -eq 'resize' })
    $last = $before | Select-Object -Last 1
    [void][GwinNative]::SetSize($hwnd, $Width, $Height)
    $deadline = (Get-Date).AddSeconds(5)
    $new = @()
    while ((Get-Date) -lt $deadline -and $new.Count -eq 0) {
      Start-Sleep -Milliseconds 200
      $new = @(Read-InputLog $state | Where-Object { $_.ev -eq 'resize' } | Select-Object -Skip $before.Count)
    }
    $shot = Save-Shot $state ('resize-{0}x{1}' -f $Width, $Height)
    $reasons = @()
    if ($new.Count -eq 0) { $reasons += 'remote_saw_no_resize' }
    $result = [ordered]@{
      step = 'resize'; ok = ($reasons.Count -eq 0); reasons = $reasons; window = $shot.rect
      remote_before = $last; remote_new = $new; shot = $shot.path
    }
    $result | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $state.run ('evidence\resize-{0}x{1}.json' -f $Width, $Height))
    Out-Line $result
  }
  'drop' {
    $state = Read-RunState
    $hellos = @(Read-InputLog $state | Where-Object { $_.ev -eq 'hello' }).Count
    $server = @(Get-OwnedChildren $state | Where-Object { $_.exe -like '*\gwin-fake-server.exe' }) | Select-Object -First 1
    if (-not $server) { Out-Line ([ordered]@{ step = 'drop'; ok = $false; reasons = @('no_owned_server') }) }
    Stop-Process -Id $server.pid -Force
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $again = $false
    while ((Get-Date) -lt $deadline) {
      Start-Sleep -Milliseconds 300
      $now = @(Read-InputLog $state | Where-Object { $_.ev -eq 'hello' }).Count
      if ($now -gt $hellos -and (Get-Streaming (Read-Stats $state))) { $again = $true; break }
    }
    $stats = Read-Stats $state
    $result = [ordered]@{
      step = 'drop'; ok = $again; killed = $server; hellos_before = $hellos
      hellos_after = @(Read-InputLog $state | Where-Object { $_.ev -eq 'hello' }).Count
      sessions = $(if ($stats) { $stats.sessions } else { $null })
    }
    New-Item -ItemType Directory -Force (Join-Path $state.run 'evidence') | Out-Null
    $result | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $state.run 'evidence\drop.json')
    Out-Line $result
  }
  'cleanup' {
    $state = Read-RunState
    $children = @(Get-OwnedChildren $state)
    $client = Get-OwnedClient $state
    $how = 'not_running'
    if ($client) {
      [void]$client.CloseMainWindow()
      if ($client.WaitForExit(10000)) { $how = 'closed' } else { Stop-Process -Id $client.Id -Force; $how = 'stopped' }
    }
    Start-Sleep -Milliseconds 500
    $ended = @()
    foreach ($c in $children) {
      $p = Get-Process -Id $c.pid -ErrorAction SilentlyContinue
      if ($p) {
        try { $path = $p.Path } catch { $path = $null }
        if ($path -eq $c.exe) { Stop-Process -Id $c.pid -Force; $ended += $c.pid }
      }
    }
    if (Test-Path $state.config) { Remove-Item -Recurse -Force $state.config }
    $kept = @(Get-ChildItem -Recurse -File $state.run | ForEach-Object { $_.FullName })
    $bye = @(Read-InputLog $state | Where-Object { $_.ev -eq 'bye' }) | Select-Object -Last 1
    Out-Line ([ordered]@{
      step = 'cleanup'; ok = ((-not (Get-OwnedClient $state)) -and (Test-Path $state.input_log))
      client = $how; children_stopped = $ended; remote_bye = $bye; evidence = $kept; run = $state.run
    })
  }
}

param(
    [Parameter(Mandatory=$true)][string]$RepoPath,
    [string]$Distro = 'Ubuntu',
    [ValidateSet('runtime-ui-demo','reader-ui-demo')][string]$Mode = 'runtime-ui-demo',
    [string]$EvidenceDir = ('/tmp/s41-native-' + [Guid]::NewGuid().ToString('N'))
)
$ErrorActionPreference = 'Stop'
if (-not $RepoPath.StartsWith('/') -or -not $EvidenceDir.StartsWith('/') -or $Distro -notmatch '^[A-Za-z0-9._-]+$') { throw 'Expected absolute WSL paths and a bounded distro name' }
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class S41Native {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    private delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr param);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr h, System.Text.StringBuilder text, int size);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetClassName(IntPtr h, System.Text.StringBuilder text, int size);
    public static IntPtr FindTarget(string title) {
        IntPtr found = IntPtr.Zero; int count = 0;
        EnumWindows((h,p) => {
            var cls = new System.Text.StringBuilder(128); GetClassName(h,cls,128);
            if (cls.ToString() != "CASCADIA_HOSTING_WINDOW_CLASS") return true;
            var text = new System.Text.StringBuilder(1024); GetWindowText(h,text,1024);
            if (text.ToString().Contains(title)) { found=h; count++; }
            return true;
        }, IntPtr.Zero);
        return count == 1 ? found : IntPtr.Zero;
    }
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    public static void Key(byte key) { keybd_event(key,0,0,UIntPtr.Zero); keybd_event(key,0,2,UIntPtr.Zero); }
    public static void Wheel() { mouse_event(0x800,0,0,120,UIntPtr.Zero); }
}
'@
if ([S41Native]::SetThreadDpiAwarenessContext([IntPtr](-4)) -eq [IntPtr]::Zero) { throw 'Physical-coordinate capture context unavailable; no input/capture permitted' }
$title = 'scramjet-rs ' + $Mode + ' synthetic ' + [Guid]::NewGuid().ToString('N')
if ([S41Native]::FindWindow($null,$title) -ne [IntPtr]::Zero) { throw 'Synthetic target title already exists; refusing ambiguous target' }
$output = '\\wsl.localhost\' + $Distro + $EvidenceDir.Replace('/','\')
if ([System.IO.Directory]::Exists($output)) { throw 'Evidence directory already exists; refusing overwrite' }
[System.IO.Directory]::CreateDirectory($output) | Out-Null
$wrapper = $RepoPath.TrimEnd('/') + '/tests/runtime_ui_native.sh'
& wt.exe -w new new-tab --title $title --suppressApplicationTitle wsl.exe -d $Distro --exec bash $wrapper $RepoPath ($EvidenceDir + '/receipt.txt') $Mode
if ($LASTEXITCODE -ne 0) { throw 'Native launcher failed' }
$h = [IntPtr]::Zero
for ($i=0; $i -lt 60; $i++) {
    $h = [S41Native]::FindTarget($title)
    if ($h -ne [IntPtr]::Zero) { break }
    Start-Sleep -Milliseconds 100
}
if ($h -eq [IntPtr]::Zero) { throw 'Isolated native target not found by exact title' }
function Focus-Target {
    [S41Native]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 100
    if ([S41Native]::GetForegroundWindow() -ne $h) { throw 'Target is not foreground; no input/capture permitted' }
}
function Key([byte]$key) {
    if ([S41Native]::GetForegroundWindow() -ne $h) { throw 'Foreground changed; input stopped' }
    [S41Native]::Key($key)
}
function Capture([string]$name) {
    if ([S41Native]::GetForegroundWindow() -ne $h) { throw 'Foreground changed; capture stopped' }
    $r = New-Object S41Native+RECT
    $p = New-Object S41Native+POINT
    if (-not [S41Native]::GetClientRect($h,[ref]$r) -or -not [S41Native]::ClientToScreen($h,[ref]$p)) { throw 'Target rectangle unavailable' }
    $w = $r.R-$r.L; $height = $r.B-$r.T
    if ($w -lt 100 -or $height -lt 100 -or $w -gt 2000 -or $height -gt 1600) { throw 'Target capture outside bounded dimensions' }
    $screen = [System.Windows.Forms.Screen]::FromHandle($h).Bounds
    if ($p.X -lt $screen.Left -or $p.Y -lt $screen.Top -or $p.X+$w -gt $screen.Right -or $p.Y+$height -gt $screen.Bottom) { throw 'Target capture is not fully on its monitor' }
    $bitmap = New-Object System.Drawing.Bitmap($w,$height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($p.X,$p.Y,0,0,(New-Object System.Drawing.Size($w,$height)))
        $bitmap.Save((Join-Path $output ($name+'.png')),[System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
    Write-Output ('captured='+$name+' target_client='+$w+'x'+$height)
}
Focus-Target
if ($Mode -eq 'reader-ui-demo') {
    [S41Native]::keybd_event(0x11,0,0,[UIntPtr]::Zero)
    try { for ($i=0; $i -lt 8; $i++) { Key 0xBD } } finally { [S41Native]::keybd_event(0x11,0,2,[UIntPtr]::Zero) }
}
[S41Native]::SetWindowPos($h,[IntPtr]::Zero,80,40,1100,680,4) | Out-Null
Start-Sleep -Milliseconds 1700
Capture '01-stream'
if ($Mode -eq 'reader-ui-demo') { Key 0x75; Key 0x74; Key 0x75 }
Key 0x73
Start-Sleep -Milliseconds 200
Capture '02-cancel'
$r = New-Object S41Native+RECT; $p = New-Object S41Native+POINT
[S41Native]::GetClientRect($h,[ref]$r) | Out-Null
[S41Native]::ClientToScreen($h,[ref]$p) | Out-Null
[S41Native]::SetCursorPos(($p.X+300),($p.Y+200)) | Out-Null
if ([S41Native]::GetForegroundWindow() -ne $h) { throw 'Foreground changed before wheel input' }
[S41Native]::Wheel()
Start-Sleep -Milliseconds 180
[S41Native]::keybd_event(0x10,0,0,[UIntPtr]::Zero)
try { Key 0x58; Key 0x59 } finally { [S41Native]::keybd_event(0x10,0,2,[UIntPtr]::Zero) }
Start-Sleep -Milliseconds 180
Capture '03-browse-edit'
Key 0x72
Key 0x09
Start-Sleep -Milliseconds 120
Capture '04-tab-transcript'
Key 0x09
if ($Mode -eq 'reader-ui-demo') {
    [S41Native]::ShowWindow($h,6) | Out-Null
    Start-Sleep -Milliseconds 180
    [S41Native]::ShowWindow($h,9) | Out-Null
} else {
    $witness = New-Object System.Windows.Forms.Form
    $witness.Text = 'S4.1 owned synthetic focus witness'
    $witness.Width=280; $witness.Height=120
    try {
        $witness.Show(); $witness.Activate()
        Start-Sleep -Milliseconds 180
    } finally { $witness.Close(); $witness.Dispose() }
}
Focus-Target
Start-Sleep -Milliseconds 120
Capture '05-focus-return'
[S41Native]::SetWindowPos($h,[IntPtr]::Zero,80,40,700,400,4) | Out-Null
Start-Sleep -Milliseconds 180
Capture '06-small'
[S41Native]::SetWindowPos($h,[IntPtr]::Zero,80,40,1100,680,4) | Out-Null
Start-Sleep -Milliseconds 180
Capture '07-restored-size'
if ([S41Native]::GetForegroundWindow() -ne $h) { throw 'Foreground changed before exit' }
[S41Native]::keybd_event(0x11,0,0,[UIntPtr]::Zero)
try { Key 0x51 } finally { [S41Native]::keybd_event(0x11,0,2,[UIntPtr]::Zero) }
Start-Sleep -Milliseconds 500
Capture '08-shell-restored'
[S41Native]::keybd_event(0x10,0,0,[UIntPtr]::Zero)
try { foreach ($key in @(0x53,0x43,0x52,0x41,0x54,0x43,0x48)) { Key $key } } finally { [S41Native]::keybd_event(0x10,0,2,[UIntPtr]::Zero) }
Key 0x0D
Start-Sleep -Milliseconds 200
Capture '09-scratch-echo'
Key 0x0D
$receipt = Join-Path $output 'receipt.txt'
if (-not [System.IO.File]::Exists($receipt)) { throw 'Native receipt missing; actions are not verification' }
$info = New-Object System.IO.FileInfo($receipt)
if ($info.Length -gt 1024) { throw 'Native receipt exceeds its metadata bound' }
$text = [System.IO.File]::ReadAllText($receipt)
if ($text -notmatch '(?m)^ui_exit=0$' -or $text -notmatch '(?m)^termios_equal=true$' -or $text -notmatch '(?m)^scratch_match=true$') { throw 'Native exit/restoration/echo receipt failed' }
Write-Output ('native_actions_completed=true clipboard_operations=0 evidence=' + $EvidenceDir)

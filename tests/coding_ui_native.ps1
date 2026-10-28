param([Parameter(Mandatory=$true)][string]$RepoPath, [string]$Distro='Ubuntu', [Parameter(Mandatory=$true)][string]$EvidenceDir)
$ErrorActionPreference='Stop'
if (-not $RepoPath.StartsWith('/') -or -not $EvidenceDir.StartsWith('/') -or $Distro -notmatch '^[A-Za-z0-9._-]+$') { throw 'Bounded absolute WSL paths required' }
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class S43Native {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X,Y; }
    private delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr param);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr h, System.Text.StringBuilder text, int size);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetClassName(IntPtr h, System.Text.StringBuilder text, int size);
    public static IntPtr Find(string title) {
        IntPtr found=IntPtr.Zero; int count=0;
        EnumWindows((h,p)=>{
            var cls=new System.Text.StringBuilder(128); GetClassName(h,cls,128);
            var text=new System.Text.StringBuilder(1024); GetWindowText(h,text,1024);
            if (cls.ToString()=="CASCADIA_HOSTING_WINDOW_CLASS" && text.ToString().Contains(title)) { found=h; count++; }
            return true;
        },IntPtr.Zero);
        return count==1 ? found : IntPtr.Zero;
    }
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int cmd);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref POINT p);
    [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
    public static void Key(byte key) { keybd_event(key,0,0,UIntPtr.Zero); keybd_event(key,0,2,UIntPtr.Zero); }
}
'@
if ([S43Native]::SetThreadDpiAwarenessContext([IntPtr](-4)) -eq [IntPtr]::Zero) { throw 'Capture coordinate context unavailable' }
$output='\\wsl.localhost\'+$Distro+$EvidenceDir.Replace('/','\')
if ([System.IO.Directory]::Exists($output)) { throw 'Evidence directory already exists' }
[System.IO.Directory]::CreateDirectory($output) | Out-Null
function Check-Focus {
    if ([S43Native]::GetForegroundWindow() -ne $script:target) { throw 'Foreground ownership changed; contaminated interval rejected' }
}
function Focus-Target {
    [S43Native]::SetForegroundWindow($script:target) | Out-Null
    Start-Sleep -Milliseconds 150
    Check-Focus
}
function Key([byte]$key) { Check-Focus; [S43Native]::Key($key) }
function Capture([string]$name) {
    Check-Focus
    $r=New-Object S43Native+RECT; $p=New-Object S43Native+POINT
    if (-not [S43Native]::GetClientRect($script:target,[ref]$r) -or -not [S43Native]::ClientToScreen($script:target,[ref]$p)) { throw 'Target rectangle unavailable' }
    $w=$r.R-$r.L; $height=$r.B-$r.T
    if ($w -lt 100 -or $height -lt 100 -or $w -gt 1600 -or $height -gt 1200) { throw 'Capture dimensions out of bounds' }
    $screen=[System.Windows.Forms.Screen]::FromHandle($script:target).Bounds
    if ($p.X -lt $screen.Left -or $p.Y -lt $screen.Top -or $p.X+$w -gt $screen.Right -or $p.Y+$height -gt $screen.Bottom) { throw 'Target not fully on monitor' }
    $bitmap=New-Object System.Drawing.Bitmap($w,$height); $graphics=[System.Drawing.Graphics]::FromImage($bitmap)
    try { $graphics.CopyFromScreen($p.X,$p.Y,0,0,(New-Object System.Drawing.Size($w,$height))); $bitmap.Save((Join-Path $output ($name+'.png')),[System.Drawing.Imaging.ImageFormat]::Png) }
    finally { $graphics.Dispose(); $bitmap.Dispose() }
    Write-Output ('captured='+$name+' client='+$w+'x'+$height)
}
foreach ($mode in @('approve','deny')) {
    $title='scramjet-rs coding-ui synthetic '+$mode+' '+[Guid]::NewGuid().ToString('N')
    $wrapper=$RepoPath.TrimEnd('/')+'/tests/coding_ui_native.sh'
    & wt.exe -w new new-tab --title $title --suppressApplicationTitle wsl.exe -d $Distro --exec bash $wrapper $RepoPath ($EvidenceDir+'/'+$mode+'-receipt.txt') ($EvidenceDir+'/'+$mode+'-ready')
    if ($LASTEXITCODE -ne 0) { throw 'Native launch failed' }
    $script:target=[IntPtr]::Zero
    for ($i=0;$i -lt 60;$i++) { $script:target=[S43Native]::Find($title); if ($script:target -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 100 }
    if ($script:target -eq [IntPtr]::Zero) { throw 'Unique synthetic target unavailable' }
    Focus-Target
    [S43Native]::keybd_event(0x11,0,0,[UIntPtr]::Zero)
    try { for ($zoom=0; $zoom -lt 8; $zoom++) { Key 0xBD } } finally { [S43Native]::keybd_event(0x11,0,2,[UIntPtr]::Zero) }
    [S43Native]::SetWindowPos($script:target,[IntPtr]::Zero,80,40,1100,680,4) | Out-Null
    Start-Sleep -Milliseconds 200
    [System.IO.File]::WriteAllText((Join-Path $output ($mode+'-ready')),'synthetic ready')
    Start-Sleep -Milliseconds 700
    Key 0x58
    Key 0x09
    Start-Sleep -Milliseconds 250
    Capture ($mode+'-01-page1')
    Key 0x78
    Start-Sleep -Milliseconds 250
    Capture ($mode+'-02-page2')
    if ($mode -eq 'deny') {
        [S43Native]::SetWindowPos($script:target,[IntPtr]::Zero,80,40,400,220,4) | Out-Null
        Start-Sleep -Milliseconds 250
        Capture ($mode+'-03-small')
        Key 0x76
        [S43Native]::SetWindowPos($script:target,[IntPtr]::Zero,80,40,1100,680,4) | Out-Null
        [S43Native]::ShowWindow($script:target,6) | Out-Null
        Start-Sleep -Milliseconds 150
        [S43Native]::ShowWindow($script:target,9) | Out-Null
        Focus-Target
        Start-Sleep -Milliseconds 250
        Capture ($mode+'-04-focus-size-return')
        Key 0x77
    } else { Key 0x76 }
    Start-Sleep -Milliseconds 800
    Capture ($mode+'-05-result')
    [S43Native]::keybd_event(0x11,0,0,[UIntPtr]::Zero)
    try { Key 0x51 } finally { [S43Native]::keybd_event(0x11,0,2,[UIntPtr]::Zero) }
    Start-Sleep -Milliseconds 400
    Capture ($mode+'-06-restored-shell')
    [S43Native]::keybd_event(0x10,0,0,[UIntPtr]::Zero)
    try { foreach ($key in @(0x53,0x43,0x52,0x41,0x54,0x43,0x48)) { Key $key } } finally { [S43Native]::keybd_event(0x10,0,2,[UIntPtr]::Zero) }
    Key 0x0D
    Start-Sleep -Milliseconds 200
    Capture ($mode+'-07-echo')
    $receipt=Join-Path $output ($mode+'-receipt.txt')
    if (-not [System.IO.File]::Exists($receipt) -or (New-Object System.IO.FileInfo($receipt)).Length -gt 2048) { throw 'Receipt missing or oversized' }
    $text=[System.IO.File]::ReadAllText($receipt)
    if ($text -notmatch '(?m)^ui_exit=0$' -or $text -notmatch '(?m)^termios_equal=true$' -or $text -notmatch '(?m)^scratch_match=true$') { throw 'Native receipt failed' }
    Key 0x0D
}
Write-Output ('native_actions_completed=true clipboard_operations=0 evidence='+$EvidenceDir)

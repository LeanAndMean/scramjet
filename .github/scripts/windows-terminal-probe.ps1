param(
    [Parameter(Mandatory=$true)][string]$Distro,
    [Parameter(Mandatory=$true)][string]$LaunchScript,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [switch]$AllowDesktopInteraction
)
$ErrorActionPreference = 'Stop'
if (-not $AllowDesktopInteraction) { throw 'Requires permission to temporarily own desktop input and overwrite the clipboard with synthetic text.' }
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ProbeDesktop {
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr window, int x, int y, int width, int height, bool repaint);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int x, int y, int data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
}
'@
[void][ProbeDesktop]::SetProcessDPIAware()
$previousWindow = [ProbeDesktop]::GetForegroundWindow()
$previousPointer = New-Object ProbeDesktop+Point
[void][ProbeDesktop]::GetCursorPos([ref]$previousPointer)
$title = "ScramjetProbe-$PID"
$window = $null
$handle = [IntPtr]::Zero
$heldButtons = 0
$report = [ordered]@{ scope = 'Windows Terminal / WSL production InteractiveMode activation journey'; checks = [ordered]@{} }
$statePath = Join-Path $OutputDirectory 'fixture.json'
$commandId = 0
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null

function State {
    if (Test-Path $statePath) { return Get-Content -Raw -Encoding UTF8 $statePath | ConvertFrom-Json }
    return $null
}
function Wait-For([scriptblock]$Predicate, [int]$Seconds = 5) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        if (& $Predicate) { return $true }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    return $false
}
function Check([string]$Name, [scriptblock]$Predicate) {
    $passed = Wait-For $Predicate
    $report.checks[$Name] = @{ passed = $passed; fixture = (State) }
    Write-Host "$Name`: $passed"
    if (-not $passed) { throw "Failed native check: $Name" }
    return $passed
}
function Fixture-Command([string]$Action) {
    $script:commandId++
    [System.IO.File]::WriteAllText("$statePath.command.tmp", (@{ id = $commandId; action = $Action } | ConvertTo-Json -Compress))
    Move-Item -Force "$statePath.command.tmp" "$statePath.command"
    if (-not (Wait-For { (State).commandDone -eq $commandId -or ($Action -eq 'suspend' -and (State).phase -eq 'suspending') -or (State).error } 10)) { throw "Fixture command did not settle: $Action" }
    if ((State).error) { throw (State).error }
}
function Assert-Focus {
    if ([ProbeDesktop]::GetForegroundWindow() -ne $handle) { throw 'Probe lost foreground focus; refusing to send input to another window.' }
}
function Key([byte]$Code, [byte[]]$Modifiers = @()) {
    Assert-Focus
    try {
        foreach ($modifier in $Modifiers) { [ProbeDesktop]::keybd_event($modifier, 0, 0, [UIntPtr]::Zero) }
        [ProbeDesktop]::keybd_event($Code, 0, 0, [UIntPtr]::Zero)
        [ProbeDesktop]::keybd_event($Code, 0, 2, [UIntPtr]::Zero)
    } finally {
        foreach ($modifier in $Modifiers) { [ProbeDesktop]::keybd_event($modifier, 0, 2, [UIntPtr]::Zero) }
    }
    Start-Sleep -Milliseconds 150
}
function Mouse([uint32]$Flags, [double]$X, [double]$Y, [int]$Data = 0) {
    Assert-Focus
    $normalizedX = [int](($X - [ProbeDesktop]::GetSystemMetrics(76)) * 65535 / ([ProbeDesktop]::GetSystemMetrics(78) - 1))
    $normalizedY = [int](($Y - [ProbeDesktop]::GetSystemMetrics(77)) * 65535 / ([ProbeDesktop]::GetSystemMetrics(79) - 1))
    [ProbeDesktop]::mouse_event(($Flags -bor 0xC001), $normalizedX, $normalizedY, $Data, [UIntPtr]::Zero)
    if ($Flags -band 2) { $script:heldButtons = $script:heldButtons -bor 4 }
    if ($Flags -band 8) { $script:heldButtons = $script:heldButtons -bor 16 }
    if ($Flags -band 4) { $script:heldButtons = $script:heldButtons -band (-bnot 4) }
    if ($Flags -band 16) { $script:heldButtons = $script:heldButtons -band (-bnot 16) }
    Start-Sleep -Milliseconds 150
}
function Cell([int]$Column, [int]$Row) {
    return @(($first[0] + ($Column - 0.5) * $cellWidth), ($first[1] + ($Row - 0.5) * $first[3]))
}
function Drag($Start, $End) {
    Mouse 2 $Start[0] $Start[1]
    for ($step = 1; $step -le 8; $step++) {
        Mouse 1 ($Start[0] + ($End[0] - $Start[0]) * $step / 8) ($Start[1] + ($End[1] - $Start[1]) * $step / 8)
    }
    Mouse 4 $End[0] $End[1]
}
function Screenshot([string]$Name) {
    Assert-Focus
    $rect = $window.Current.BoundingRectangle
    $bitmap = New-Object System.Drawing.Bitmap ([int]$rect.Width - 24), ([int]$rect.Height - 24)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen(([int]$rect.X + 12), ([int]$rect.Y + 12), 0, 0, $bitmap.Size)
        $bitmap.Save((Join-Path $OutputDirectory "$Name.png"))
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
try {
    $report.os = [Environment]::OSVersion.VersionString
    $report.terminalVersion = (Get-AppxPackage Microsoft.WindowsTerminal).Version.ToString()
    $wt = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\wt.exe'
    & $wt -w new new-tab --title $title --suppressApplicationTitle wsl.exe -d $Distro -- bash --noprofile --norc
    $condition = New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::NameProperty), $title
    if (-not (Wait-For { $script:window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition); $null -ne $script:window } 20)) {
        throw 'Owned Windows Terminal window was not found'
    }
    $handle = [IntPtr]$window.Current.NativeWindowHandle
    [void][ProbeDesktop]::SetForegroundWindow($handle)
    Start-Sleep -Seconds 2
    Assert-Focus
    [System.Windows.Forms.SendKeys]::SendWait("bash '$LaunchScript'")
    Key 13
    if (-not (Wait-For { $null -ne (State) } 20)) { throw 'WSL fixture did not start' }
    $textCondition = New-Object System.Windows.Automation.PropertyCondition ([System.Windows.Automation.AutomationElement]::ClassNameProperty), 'TermControl'
    $textElement = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $textCondition)
    if ($null -eq $textElement) { throw 'Terminal text accessibility pattern unavailable' }
    $rect = $textElement.Current.BoundingRectangle
    $columns = (State).columns
    $rows = (State).rows
    [void](Check 'productionCompositionConfigured' { (State).production -eq $true -and (State).journey -eq $true -and (State).totalRows -gt 240 })
    $cellWidth = [Math]::Floor($rect.Width / $columns)
    $first = @(($rect.X + ($rect.Width % $columns) / 2), ($rect.Y + ($rect.Height % $rows) / 2), 0, ([Math]::Floor($rect.Height / $rows)))
    if ($cellWidth -le 0 -or $first[3] -le 0) { throw 'Terminal grid bounds unavailable' }
    $report.initialGrid = @{ x = $first[0]; y = $first[1]; cellWidth = $cellWidth; cellHeight = $first[3] }
    $point = Cell 10 3
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    if (-not (Wait-For { $null -ne (State).lastMouse })) { throw 'Desktop calibration click did not reach the fixture' }
    $observed = (State).lastMouse
    $first[0] += (10 - $observed.x) * $cellWidth
    $first[1] += (3 - $observed.y) * $first[3]
    $point = Cell 10 3
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    if (-not (Check 'desktopCellTargetVerified' { (State).lastMouse.x -eq 10 -and (State).lastMouse.y -eq 3 })) { throw 'Desktop targeting remains uncalibrated' }
    [System.Windows.Forms.Clipboard]::SetText('SCRAMJET-PROBE-SENTINEL')
    Screenshot 'startup'
    $point = Cell 10 3
    Mouse 2048 $point[0] $point[1] -360
    [void](Check 'desktopWheelScrollsDocument' { (State).wheel -gt 0 -and (State).offset -gt 0 })
    Screenshot 'wheel'
    Drag (Cell $columns 1) (Cell $columns $rows)
    [void](Check 'desktopThumbDragReachesEnd' { (State).thumbDrag -gt 0 -and (State).followingTail -eq $true -and (State).offset -eq ((State).totalRows - (State).height) })
    $point = Cell $columns 1
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    [void](Check 'desktopTrackClickReachesStart' { (State).offset -eq 0 -and (State).thumbDrag -gt 0 })
    Drag (Cell 1 1) (Cell 60 1)
    [void](Check 'ordinaryDesktopDragSelects' { (State).selectionDrag -gt 0 })
    Screenshot 'selection'
    $expected = 'ROW-001 synthetic caf' + [char]0xE9 + ' ' + [char]0x754C + ' e' + [char]0x301 + ' text'
    $point = Cell 10 1
    Mouse 8 $point[0] $point[1]
    Mouse 16 $point[0] $point[1]
    [void](Check 'rightClickRequestsCopy' { (State).rightCopy -gt 0 })
    [void](Check 'rightClickClipboardExactUnicode' { [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $expected, [StringComparison]::Ordinal) })
    Screenshot 'right-click'
    Mouse 8 $point[0] $point[1]
    Mouse 16 $point[0] $point[1]
    [void](Check 'rightWithoutSelectionDoesNotCopyOrPaste' { (State).rightWithoutSelection -gt 0 -and (State).rightCopy -eq 1 -and (State).editor -ceq 'Synthetic editor' })
    Drag (Cell 1 1) (Cell 60 1)
    [System.Windows.Forms.Clipboard]::SetText('SCRAMJET-PROBE-SENTINEL')
    Key 67 @(17)
    [void](Check 'controlCCopiesSelection' { (State).keyCopy -gt 0 -and [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $expected, [StringComparison]::Ordinal) })
    Key 86 @(17, 16)
    [void](Check 'desktopPasteRoundTrip' { (State).pasteMatches -gt 0 })
    Fixture-Command 'editor'
    foreach ($code in @(65, 66, 67, 37, 8)) { Key $code }
    [void](Check 'keyboardEditingCoexists' { (State).editor -ceq 'ac' })
    Screenshot 'keyboard'
    Fixture-Command 'expand'
    $point = Cell $columns ([int]($rows / 2))
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    [void](Check 'longSessionMiddleReachable' { $ratio = (State).offset / ((State).totalRows - (State).height); $ratio -gt 0.3 -and $ratio -lt 0.7 })
    $point = Cell $columns 1
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    $point = Cell 1 3
    Mouse 2 $point[0] $point[1]
    $point = Cell 60 ($rows - 1)
    Mouse 1 $point[0] $point[1]
    Start-Sleep -Milliseconds 500
    Mouse 4 $point[0] $point[1]
    [void](Check 'selectionAutoscrolls' { (State).offset -gt 0 -and (State).notice })
    $lastSelected = (State).painted[$rows - 2]
    if (-not $lastSelected.StartsWith('ROW-')) { throw 'Selection escaped synthetic history' }
    $lastNumber = [int]$lastSelected.Substring(4, 3)
    $suffix = $expected.Substring(7)
    $expectedMultiline = ((2..$lastNumber | ForEach-Object { 'ROW-' + $_.ToString('000') + $suffix }) -join "`n")
    Fixture-Command 'update'
    [void](Check 'selectionHoldsDuringUpdates' { (State).notice -ceq 'updates pending; Esc clears' })
    Screenshot 'selection-across-scroll'
    Key 67 @(17)
    [void](Check 'scrolledSelectionClipboardExact' { [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $expectedMultiline, [StringComparison]::Ordinal) -and -not (State).notice })
    function Browse-Cards([int]$Count) {
        $seen = New-Object 'System.Collections.Generic.HashSet[int]'
        $point = Cell $columns $rows
        Mouse 2 $point[0] $point[1]
        Mouse 4 $point[0] $point[1]
        for ($step = 0; $step -lt 180; $step++) {
            foreach ($line in (State).painted) {
                for ($i = 1; $i -le $Count; $i++) { if ($line.Contains("CARD-$i ")) { [void]$seen.Add($i) } }
            }
            if ($seen.Count -eq $Count -or (State).offset -eq 0) { break }
            $point = Cell 10 3
            Mouse 2048 $point[0] $point[1] 120
        }
        return $seen.Count
    }
    $seen = Browse-Cards 4
    [void](Check 'firstFourRunningCardsReachable' { $seen -eq 4 -and (State).completed -eq 0 })
    for ($i = 0; $i -lt 4; $i++) { Fixture-Command 'advance' }
    $seen = Browse-Cards 8
    [void](Check 'allEightCardsReachableBeforeCompletion' { $seen -eq 8 -and (State).completed -eq 4 })
    $point = Cell $columns $rows
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    for ($step = 0; $step -lt 160; $step++) {
        if ((State).painted[0].StartsWith(' child-3 detail-')) { break }
        $point = Cell 10 3
        Mouse 2048 $point[0] $point[1] 120
    }
    $anchor = (State).painted[0]
    [void](Check 'readingInsideRunningBatch' { $anchor.StartsWith(' child-3 detail-') })
    Fixture-Command 'update'
    [void](Check 'readingAnchorSurvivesOtherChildUpdate' { (State).painted[0] -ceq $anchor -and -not (State).followingTail })
    $windowRect = $window.Current.BoundingRectangle
    Assert-Focus
    [void][ProbeDesktop]::MoveWindow($handle, [int]$windowRect.X, [int]$windowRect.Y, ([int]$windowRect.Width - 120), ([int]$windowRect.Height - 60), $true)
    [void](Check 'nativeWidthAndHeightChanged' { (State).columns -lt $columns -and (State).rows -lt $rows })
    [void](Check 'readingAnchorSurvivesResize' { (State).painted[0] -ceq $anchor })
    Screenshot 'resized-reading'
    [void][ProbeDesktop]::MoveWindow($handle, [int]$windowRect.X, [int]$windowRect.Y, [int]$windowRect.Width, [int]$windowRect.Height, $true)
    [void](Check 'nativeSizeRestored' { (State).columns -eq $columns -and (State).rows -eq $rows })
    [void](Check 'readingAnchorSurvivesResizeBack' { (State).painted[0] -ceq $anchor })
    for ($i = 0; $i -lt 4; $i++) { Fixture-Command 'advance' }
    Fixture-Command 'approval'
    $payloadSeen = New-Object 'System.Collections.Generic.HashSet[int]'
    $point = Cell $columns $rows
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    for ($step = 0; $step -lt 100; $step++) {
        foreach ($line in (State).painted) {
            if ($line -match '^IMMUTABLE-SYNTHETIC-PAYLOAD-(\d+)$') { [void]$payloadSeen.Add([int]$Matches[1]) }
        }
        if ($payloadSeen.Count -eq 60) { break }
        $point = Cell 10 3
        Mouse 2048 $point[0] $point[1] 120
    }
    [void](Check 'completeApprovalContextReachable' { $payloadSeen.Count -eq 60 })
    $point = Cell $columns 1
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    Key 13
    [void](Check 'hiddenApprovalActivationOnlyReveals' { (State).approved -eq 0 -and @((State).painted | Where-Object { $_.Contains('SYNTHETIC APPROVAL') }).Count -eq 1 })
    Key 13
    [void](Check 'subsequentApprovalActivation' { (State).approved -eq 1 })
    Fixture-Command 'external'
    [void](Check 'externalProgramRoundTrip' { (State).editorHandoffs -eq 1 -and (State).handoffTermios -ceq (State).termiosBefore -and (State).editor -ceq 'edited by synthetic external editor' })
    Fixture-Command 'suspend'
    [void](Check 'jobControlSuspended' { $status = & wsl.exe -d $Distro -- ps -o stat= -p ([string](State).pid); $status.Contains('T') })
    Screenshot 'suspended-shell'
    Key 70
    Key 71
    Key 13
    [void](Check 'jobControlResumed' { (State).phase -eq 'resumed' })
    Key 81 @(17)
    [void](Check 'orderlyExit' { (State).stopped -eq $true -and (Test-Path (Join-Path $OutputDirectory 'stty-after.txt')) })
    [void](Check 'termiosRestored' { (State).termiosBefore -and (State).termiosBefore -ceq (State).termiosAfter })
    Screenshot 'restored'
} catch {
    $report.error = $_.ToString()
    if ($handle -ne [IntPtr]::Zero -and [ProbeDesktop]::GetForegroundWindow() -eq $handle) { Screenshot 'failure' }
} finally {
    if ($heldButtons) { [ProbeDesktop]::mouse_event($heldButtons, 0, 0, 0, [UIntPtr]::Zero) }
    if ($null -ne $window) {
        try {
            if ([ProbeDesktop]::GetForegroundWindow() -eq $handle) { Key 81 @(17) }
            $window.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close()
        } catch { $report.cleanupError = $_.ToString() }
    }
    [void][ProbeDesktop]::SetCursorPos($previousPointer.X, $previousPointer.Y)
    [void][ProbeDesktop]::SetForegroundWindow($previousWindow)
    $report.passed = $report.checks.Count -eq 32 -and @($report.checks.Values | Where-Object { -not $_.passed }).Count -eq 0 -and -not $report.Contains('error') -and -not $report.Contains('cleanupError')
    [System.IO.File]::WriteAllText((Join-Path $OutputDirectory 'report.json'), ($report | ConvertTo-Json -Depth 10))
    $report | ConvertTo-Json -Depth 10
}
if (-not $report.passed) { exit 1 }

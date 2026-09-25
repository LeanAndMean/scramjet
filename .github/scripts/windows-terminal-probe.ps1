param(
    [Parameter(Mandatory=$true)][string]$Distro,
    [Parameter(Mandatory=$true)][string]$LaunchScript,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [Parameter(Mandatory=$true)][string]$SourceRevision,
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
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr window);
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
if ($previousWindow -eq [IntPtr]::Zero -or -not [ProbeDesktop]::GetCursorPos([ref]$previousPointer)) { throw 'Cannot capture desktop state for later restoration.' }
$title = "ScramjetProbe-$PID"
$window = $null
$handle = [IntPtr]::Zero
$heldButtons = 0
$report = [ordered]@{ scope = 'Windows Terminal / WSL production InteractiveMode activation journey'; checks = [ordered]@{} }
$statePath = Join-Path $OutputDirectory 'fixture.json'
$commandId = 0
$requiredChecks = @(
    'productionCompositionConfigured', 'desktopCellTargetVerified', 'desktopWheelScrollsDocument',
    'desktopThumbDragReachesEnd', 'desktopTrackClickReachesStart', 'ordinaryDesktopDragSelects',
    'rightClickRequestsCopy', 'rightClickClipboardExactUnicode', 'rightWithoutSelectionDoesNotCopyOrPaste',
    'controlCCopiesSelection', 'desktopPasteRoundTrip', 'keyboardEditingCoexists', 'longSessionMiddleReachable',
    'selectionAutoscrolls', 'selectionAllowsLiveUpdates', 'scrolledSelectionClipboardExact', 'editorRightClickPastesWithoutSubmit',
    'firstFourRunningCardsReachable', 'allEightCardsReachableBeforeCompletion', 'readingInsideRunningBatch',
    'readingAnchorSurvivesOtherChildUpdate', 'nativeWidthAndHeightChanged', 'readingAnchorSurvivesResize',
    'nativeSizeRestored', 'readingAnchorSurvivesResizeBack', 'completeApprovalContextReachable',
    'hiddenApprovalActivationOnlyReveals', 'subsequentApprovalActivation', 'externalProgramRoundTrip',
    'jobControlSuspended', 'jobControlResumed', 'orderlyExit', 'termiosRestored', 'checkoutProvenanceMatches',
    'defaultDockKeepsInputVisible', 'dockedTypingPreservesReading', 'keyboardOnlyBrowsingFromTail',
    'keyboardBrowsingReturnsToTail', 'nativePresentationTogglePreservesReading', 'settingsUndocksLive',
    'settingsRedocksLive', 'settingsWheelChangeApplies', 'configuredWheelDistance',
    'settingsEditorHeightChangeApplies', 'nativeInputHeightCeiling',
    'heldWheelDownCopiesExact', 'heldWheelUpCopiesExact', 'heldWheelReversalCopiesExact',
    'mixedWheelDownCopiesExact', 'mixedWheelUpCopiesExact', 'mixedWheelReversalCopiesExact',
    'editorHomeEndStable', 'transcriptControlHomeEnd', 'selectionKeepsLayout',
    'newUserMessageFollowsTail', 'copyOmitsPaddingAndSoftWraps', 'editorCopyOmitsSoftWraps',
    'selectionCrossesIntoEditor', 'selectionCrossesIntoTranscript'
)
$report.sourceRevision = $SourceRevision
$report.viewportKeys = 'Alt+PageUp/Alt+PageDown'
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
function Check([string]$Name, [scriptblock]$Predicate, [int]$StableMilliseconds = 0) {
    if (-not ($requiredChecks -ccontains $Name) -or $report.checks.Contains($Name)) { throw "Unexpected or duplicate native check: $Name" }
    $passed = Wait-For $Predicate
    $deadline = [DateTime]::UtcNow.AddMilliseconds($StableMilliseconds)
    while ($passed -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 50
        $passed = [bool](& $Predicate)
    }
    $report.checks[$Name] = @{ passed = $passed; fixture = (State) }
    Write-Host "$Name`: $passed"
    if (-not $passed) { throw "Failed native check: $Name" }
    return $passed
}
function Right-ClickUnchanged($Before) {
    $current = State
    if ($current.rightWithoutSelection -ne ($Before.rightWithoutSelection + 1)) { return $false }
    foreach ($name in @('rightCopy', 'keyCopy', 'copyErrors', 'pasteMatches', 'pasteMismatches', 'editor')) {
        if ($current.$name -cne $Before.$name) { return $false }
    }
    return $true
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
function Key([byte]$Code, [byte[]]$Modifiers = @(), [switch]$Extended) {
    Assert-Focus
    try {
        foreach ($modifier in $Modifiers) { [ProbeDesktop]::keybd_event($modifier, 0, 0, [UIntPtr]::Zero) }
        $flags = if ($Extended) { 1 } else { 0 }
        [ProbeDesktop]::keybd_event($Code, 0, $flags, [UIntPtr]::Zero)
        [ProbeDesktop]::keybd_event($Code, 0, ($flags -bor 2), [UIntPtr]::Zero)
    } finally {
        foreach ($modifier in $Modifiers) { [ProbeDesktop]::keybd_event($modifier, 0, 2, [UIntPtr]::Zero) }
    }
    Start-Sleep -Milliseconds 150
}
function Mouse([uint32]$Flags, [double]$X, [double]$Y, [int]$Data = 0, [int]$PauseMilliseconds = 150) {
    Assert-Focus
    $normalizedX = [int](($X - [ProbeDesktop]::GetSystemMetrics(76)) * 65535 / ([ProbeDesktop]::GetSystemMetrics(78) - 1))
    $normalizedY = [int](($Y - [ProbeDesktop]::GetSystemMetrics(77)) * 65535 / ([ProbeDesktop]::GetSystemMetrics(79) - 1))
    [ProbeDesktop]::mouse_event(($Flags -bor 0xC001), $normalizedX, $normalizedY, $Data, [UIntPtr]::Zero)
    if ($Flags -band 2) { $script:heldButtons = $script:heldButtons -bor 4 }
    if ($Flags -band 8) { $script:heldButtons = $script:heldButtons -bor 16 }
    if ($Flags -band 4) { $script:heldButtons = $script:heldButtons -band (-bnot 4) }
    if ($Flags -band 16) { $script:heldButtons = $script:heldButtons -band (-bnot 16) }
    if ($PauseMilliseconds -gt 0) { Start-Sleep -Milliseconds $PauseMilliseconds }
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
function Stationary-Wheel([int]$Delta, [int]$PauseMilliseconds = 150) {
    Assert-Focus
    [ProbeDesktop]::mouse_event(2048, 0, 0, $Delta, [UIntPtr]::Zero)
    if ($PauseMilliseconds -gt 0) { Start-Sleep -Milliseconds $PauseMilliseconds }
}
function Check-HeldWheelSelections([string]$Suffix) {
    foreach ($scenario in @(
        @{ name = 'heldWheelDownCopiesExact'; steps = @(1,1,1,1,1,1,1,1) },
        @{ name = 'heldWheelUpCopiesExact'; steps = @(-1,-1,-1,-1,-1,-1,-1,-1) },
        @{ name = 'heldWheelReversalCopiesExact'; steps = @(1,1,1,1,-1,-1,-1,-1,-1,-1) },
        @{ name = 'mixedWheelDownCopiesExact'; steps = @(1,1,1,1,1,1,1,1); mixed = $true; wheelFirst = $false },
        @{ name = 'mixedWheelUpCopiesExact'; steps = @(-1,-1,-1,-1,-1,-1,-1,-1); mixed = $true; wheelFirst = $true },
        @{ name = 'mixedWheelReversalCopiesExact'; steps = @(1,1,1,1,-1,-1,-1,-1,-1,-1); mixed = $true; wheelFirst = $true }
    )) {
        $point = Cell $columns 1
        Mouse 2 $point[0] $point[1]
        Mouse 4 $point[0] $point[1]
        $point = Cell 4 3
        Mouse 1 $point[0] $point[1]
        $beforeReading = State
        for ($i = 0; $i -lt 12; $i++) { Stationary-Wheel -120 }
        if (-not (Wait-For { (State).wheel -eq ($beforeReading.wheel + 12) -and (State).frameFlushed -eq $true -and (State).offset -gt 0 })) { throw 'Held-wheel starting frame did not flush' }
        $origin = State
        if ($origin.painted[2] -notmatch '^ROW-(\d{3}) ') { throw 'Held-wheel selection must begin in synthetic history' }
        $startNumber = [int]$Matches[1]
        Mouse 2 $point[0] $point[1]
        $point = Cell 5 3
        Mouse 1 $point[0] $point[1]
        if (-not (Wait-For { (State).selectionDrag -gt $origin.selectionDrag -and (State).frameFlushed -eq $true })) { throw 'Initial selection drag did not settle' }
        $drag = State
        $delta = ($scenario.steps | Measure-Object -Sum).Sum * $drag.wheelStep
        for ($index = 0; $index -lt $scenario.steps.Count; $index++) {
            $step = $scenario.steps[$index]
            if ($scenario.mixed) {
                $point = Cell (5 + $index % 4) (2 + $index % 3)
                if ($scenario.wheelFirst) {
                    Stationary-Wheel (-120 * $step) 0
                    Mouse 1 $point[0] $point[1]
                } else {
                    Mouse 1 $point[0] $point[1] 0 0
                    Stationary-Wheel (-120 * $step)
                }
            } else { Stationary-Wheel (-120 * $step) }
        }
        if (-not (Wait-For { (State).wheel -eq ($drag.wheel + $scenario.steps.Count) -and (State).offset -eq ($drag.offset + $delta) -and (State).frameFlushed -eq $true })) { throw 'Held wheel did not reach its expected position' }
        if ($scenario.mixed) {
            if ((State).selectionDrag -le $drag.selectionDrag) { throw 'Mixed-input selection received no drag motion' }
        } elseif ((State).selectionDrag -ne $drag.selectionDrag) { throw 'Pointer motion contaminated the stationary-wheel interval' }
        Screenshot "$($scenario.name)-held"
        if ($scenario.mixed) {
            $point = Cell 7 4
            Mouse 1 $point[0] $point[1] 0 0
        }
        Mouse 4 $point[0] $point[1]
        if (-not (Wait-For { (State).lastMouse.action -ceq 'm' -and (State).frameFlushed -eq $true })) { throw 'Selection release did not settle' }
        Screenshot "$($scenario.name)-released"
        $released = State
        for ($i = 0; $i -lt 4; $i++) { Stationary-Wheel -120 }
        if (-not (Wait-For { (State).offset -eq ($released.offset + 4 * $drag.wheelStep) -and (State).frameFlushed -eq $true })) { throw 'Released-selection wheel did not settle' }
        Screenshot "$($scenario.name)-released-scroll"
        $endNumber = $startNumber + $delta + $(if ($scenario.mixed) { 1 } else { 0 })
        $endColumn = if ($scenario.mixed) { 6 } else { 4 }
        $firstNumber = [Math]::Min($startNumber, $endNumber)
        $lastNumber = [Math]::Max($startNumber, $endNumber)
        $firstColumn = if ($endNumber -gt $startNumber) { 3 } else { $endColumn }
        $lastColumn = if ($endNumber -gt $startNumber) { $endColumn } else { 3 }
        $parts = @((('ROW-' + $firstNumber.ToString('000') + $Suffix).Substring($firstColumn)))
        for ($i = $firstNumber + 1; $i -lt $lastNumber; $i++) { $parts += 'ROW-' + $i.ToString('000') + $Suffix }
        $parts += ('ROW-' + $lastNumber.ToString('000') + $Suffix).Substring(0, $lastColumn)
        $expectedSelection = $parts -join "`n"
        [System.Windows.Forms.Clipboard]::SetText('SCRAMJET-PROBE-SENTINEL')
        Mouse 8 $point[0] $point[1]
        Mouse 16 $point[0] $point[1]
        [void](Check $scenario.name { [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $expectedSelection, [StringComparison]::Ordinal) -and -not (State).selectionActive })
    }
}
function Open-Settings([string]$Query) {
    Fixture-Command 'editor'
    Assert-Focus
    [System.Windows.Forms.SendKeys]::SendWait('/settings')
    Key 13
    if (-not (Wait-For { @((State).painted | Where-Object { $_.Contains('Auto-compact') }).Count -gt 0 })) { throw 'Real settings selector did not open' }
    Assert-Focus
    [System.Windows.Forms.SendKeys]::SendWait($Query)
    if (-not (Wait-For { $current = State; $current.frameFlushed -eq $true -and @($current.painted | Where-Object { $_.Trim() -ceq "> $Query" }).Count -gt 0 })) { throw 'Complete settings search did not flush' }
}
function Close-Settings {
    Key 27
    if (-not (Wait-For { (State).editorActive -eq $true -and (State).frameFlushed -eq $true })) { throw 'Settings selector did not release focus after Escape' }
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
function Cleanup-OwnedResources {
    $errors = New-Object 'System.Collections.Generic.List[string]'
    $cleanup = [ordered]@{ windowClosed = ($null -eq $window); pointerRestored = $false; focusRestored = $false }
    try {
        if ($heldButtons) { [ProbeDesktop]::mouse_event($heldButtons, 0, 0, 0, [UIntPtr]::Zero) }
    } catch { $errors.Add("Mouse release: $($_.Exception.Message)") }
    if ($null -ne $window) {
        try {
            if ([ProbeDesktop]::GetForegroundWindow() -eq $handle) { Key 81 @(17) }
        } catch { $errors.Add("Exit key: $($_.Exception.Message)") }
        try {
            $window.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close()
        } catch { $errors.Add("Window close: $($_.Exception.Message)") }
        try {
            $cleanup.windowClosed = Wait-For { -not [ProbeDesktop]::IsWindow($handle) }
            if (-not $cleanup.windowClosed) { $errors.Add('Owned terminal window remains open') }
        } catch { $errors.Add("Window verification: $($_.Exception.Message)") }
    }
    try {
        if (-not [ProbeDesktop]::SetCursorPos($previousPointer.X, $previousPointer.Y)) { $errors.Add('Pointer restoration request failed') }
        $actualPointer = New-Object ProbeDesktop+Point
        $cleanup.pointerRestored = [ProbeDesktop]::GetCursorPos([ref]$actualPointer) -and $actualPointer.X -eq $previousPointer.X -and $actualPointer.Y -eq $previousPointer.Y
        if (-not $cleanup.pointerRestored) { $errors.Add('Pointer restoration was not verified') }
    } catch { $errors.Add("Pointer restoration: $($_.Exception.Message)") }
    try {
        [void][ProbeDesktop]::SetForegroundWindow($previousWindow)
        $cleanup.focusRestored = Wait-For { [ProbeDesktop]::GetForegroundWindow() -eq $previousWindow }
        if (-not $cleanup.focusRestored) { $errors.Add('Foreground restoration was not verified') }
    } catch { $errors.Add("Foreground restoration: $($_.Exception.Message)") }
    $report.desktopCleanup = $cleanup
    if ($errors.Count) { $report.cleanupError = $errors -join '; ' }
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
    [void](Check 'checkoutProvenanceMatches' { (State).sourceRevision -ceq $SourceRevision -and (State).sourceDirty -eq $false })
    [void](Check 'defaultDockKeepsInputVisible' { (State).dockEditor -eq $true -and (State).height -lt (State).rows -and @((State).painted | Where-Object { $_.Contains('Synthetic editor') }).Count -gt 0 })
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
    Drag (Cell $columns 1) (Cell $columns (State).height)
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
    $beforeRight = State
    Mouse 8 $point[0] $point[1]
    Mouse 16 $point[0] $point[1]
    if (-not (Wait-For { $current = State; $current.rightWithoutSelection -eq ($beforeRight.rightWithoutSelection + 1) -and $current.frameFlushed -eq $true })) { throw 'No-selection right click did not reach a flushed frame' }
    [void](Check 'rightWithoutSelectionDoesNotCopyOrPaste' { Right-ClickUnchanged $beforeRight } 350)
    Drag (Cell 1 1) (Cell 60 1)
    [System.Windows.Forms.Clipboard]::SetText('SCRAMJET-PROBE-SENTINEL')
    Key 67 @(17)
    [void](Check 'controlCCopiesSelection' { (State).keyCopy -gt 0 -and [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $expected, [StringComparison]::Ordinal) })
    Key 86 @(17, 16)
    [void](Check 'desktopPasteRoundTrip' { (State).pasteMatches -gt 0 })
    $pasteBefore = State
    $pasteText = "RIGHT-PASTE caf$([char]0xE9) $([char]0x754C)`nsecond line"
    [System.Windows.Forms.Clipboard]::SetText($pasteText)
    $editorRow = @(0..($pasteBefore.painted.Count - 1) | Where-Object { $pasteBefore.painted[$_].Contains('Synthetic editor') })[0]
    if ($null -eq $editorRow) { throw 'Paste target editor is not painted' }
    $point = Cell 3 ($editorRow + 1)
    Mouse 8 $point[0] $point[1]
    Mouse 16 $point[0] $point[1]
    [void](Check 'editorRightClickPastesWithoutSubmit' { (State).editor -ceq ($pasteBefore.editor + $pasteText) -and (State).submissions -eq $pasteBefore.submissions })
    Screenshot 'editor-right-paste'
    Check-HeldWheelSelections $expected.Substring(7)
    Fixture-Command 'editor'
    foreach ($code in @(65, 66, 67, 37, 8)) { Key $code }
    [void](Check 'keyboardEditingCoexists' { (State).editor -ceq 'ac' })
    $beforeEditing = State
    Key 36 @() -Extended
    Key 88
    Key 35 @() -Extended
    Key 89
    [void](Check 'editorHomeEndStable' { (State).editor -ceq 'xacy' -and (State).offset -eq $beforeEditing.offset })
    Key 36 @(17) -Extended
    if (-not (Wait-For { (State).offset -eq 0 -and (State).followingTail -eq $false -and (State).frameFlushed -eq $true })) { throw 'Ctrl+Home did not reach the transcript beginning' }
    Key 35 @(17) -Extended
    [void](Check 'transcriptControlHomeEnd' { (State).followingTail -eq $true -and (State).offset -eq ((State).totalRows - (State).height) })
    Key 36 @(17) -Extended
    $beforeSelection = State
    Drag (Cell 1 3) (Cell 60 3)
    [void](Check 'selectionKeepsLayout' { (State).selectionActive -and (State).height -eq $beforeSelection.height -and (State).offset -eq $beforeSelection.offset -and ((State).painted -join "`n") -ceq ($beforeSelection.painted -join "`n") })
    Key 67 @(17)
    Screenshot 'keyboard'
    Fixture-Command 'editor'
    $point = Cell $columns 1
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    $readingOffset = (State).offset
    Key 65
    [void](Check 'dockedTypingPreservesReading' { (State).editor -ceq 'a' -and (State).offset -eq $readingOffset -and (State).painted[0].StartsWith('ROW-001') })
    Fixture-Command 'tail'
    $tail = State
    Key 33 @(18) -Extended
    [void](Check 'keyboardOnlyBrowsingFromTail' { (State).followingTail -eq $false -and (State).offset -eq ($tail.offset - $tail.height) })
    Key 34 @(18) -Extended
    [void](Check 'keyboardBrowsingReturnsToTail' { (State).followingTail -eq $true -and (State).offset -eq $tail.offset })
    $point = Cell $columns 1
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    $anchor = (State).painted[0]
    $expanded = (State).toolsExpanded
    Key 79 @(17)
    [void](Check 'nativePresentationTogglePreservesReading' { (State).toolsExpanded -ne $expanded -and (State).painted[0] -ceq $anchor -and (State).followingTail -eq $false })
    Open-Settings 'dock'
    Key 13
    [void](Check 'settingsUndocksLive' { (State).dockEditor -eq $false -and (State).height -eq (State).rows })
    Close-Settings
    Open-Settings 'dock'
    Key 13
    [void](Check 'settingsRedocksLive' { (State).dockEditor -eq $true -and (State).height -lt (State).rows })
    Close-Settings
    Open-Settings 'wheel'
    Key 13
    [void](Check 'settingsWheelChangeApplies' { (State).wheelStep -eq 4 })
    Close-Settings
    $point = Cell $columns ([int]((State).height / 2))
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    $beforeWheel = State
    $point = Cell 10 3
    Mouse 2048 $point[0] $point[1] 120
    [void](Check 'configuredWheelDistance' { (State).wheel -gt $beforeWheel.wheel -and (State).offset -eq ($beforeWheel.offset - 4 * ((State).wheel - $beforeWheel.wheel)) } 350)
    Open-Settings 'height'
    Key 13
    [void](Check 'settingsEditorHeightChangeApplies' { (State).editorHeightPercent -eq 35 })
    Close-Settings
    Fixture-Command 'long-editor'
    [void](Check 'nativeInputHeightCeiling' { @((State).painted | Where-Object { $_.Trim().StartsWith('INPUT-') }).Count -eq [Math]::Floor((State).rows * 0.35) })
    Screenshot 'docked-settings'
    Fixture-Command 'editor'
    Fixture-Command 'expand'
    $point = Cell $columns ([int]((State).height / 2))
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    [void](Check 'longSessionMiddleReachable' { $ratio = (State).offset / ((State).totalRows - (State).height); $ratio -gt 0.3 -and $ratio -lt 0.7 })
    $point = Cell $columns 1
    Mouse 2 $point[0] $point[1]
    Mouse 4 $point[0] $point[1]
    $point = Cell 1 3
    Mouse 2 $point[0] $point[1]
    $point = Cell 60 (State).height
    Mouse 1 $point[0] $point[1]
    Start-Sleep -Milliseconds 500
    Mouse 4 $point[0] $point[1]
    [void](Check 'selectionAutoscrolls' { (State).offset -gt 0 -and (State).selectionActive })
    $lastSelected = (State).painted[(State).height - 1]
    if (-not $lastSelected.StartsWith('ROW-')) { throw 'Selection escaped synthetic history' }
    $lastNumber = [int]$lastSelected.Substring(4, 3)
    $suffix = $expected.Substring(7)
    $expectedMultiline = ((2..$lastNumber | ForEach-Object { 'ROW-' + $_.ToString('000') + $suffix }) -join "`n")
    $heldFrame = State
    Fixture-Command 'update'
    [void](Check 'selectionAllowsLiveUpdates' { (State).selectionActive -and (State).updates -gt $heldFrame.updates -and ((State).painted -join "`n").Contains("LIVE-UPDATES-$((State).updates)") -and ((State).painted -join "`n") -cne ($heldFrame.painted -join "`n") })
    Screenshot 'selection-across-scroll'
    Key 67 @(17)
    [void](Check 'scrolledSelectionClipboardExact' { [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $expectedMultiline, [StringComparison]::Ordinal) -and -not (State).selectionActive })
    function Browse-Cards([int]$Count) {
        $seen = New-Object 'System.Collections.Generic.HashSet[int]'
        $point = Cell $columns (State).height
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
    $point = Cell $columns (State).height
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
    $point = Cell $columns (State).height
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
    $enterCount = (State).enterPresses
    Key 13
    [void](Check 'hiddenApprovalActivationOnlyReveals' { (State).enterPresses -gt $enterCount -and (State).frameFlushed -eq $true -and (State).approved -eq 0 -and @((State).painted | Where-Object { $_.Contains('SYNTHETIC APPROVAL') }).Count -eq 1 } 350)
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
    Key 36 @(17) -Extended
    Fixture-Command 'copy-prose'
    [void](Check 'newUserMessageFollowsTail' { (State).followingTail -eq $true -and @((State).painted | Where-Object { $_.Contains('COPY-PROSE') }).Count -eq 1 })
    $copyFrame = State
    $startRow = @(0..($copyFrame.painted.Count - 1) | Where-Object { $copyFrame.painted[$_].Contains('COPY-PROSE') })[0]
    $endRow = @(0..($copyFrame.painted.Count - 1) | Where-Object { $copyFrame.painted[$_].Trim() -ceq '```' })[-1]
    if ($null -eq $startRow -or $null -eq $endRow -or $endRow -le $startRow) { throw 'Copy prose and code were not fully visible' }
    [System.Windows.Forms.Clipboard]::SetText('SCRAMJET-PROBE-SENTINEL')
    Drag (Cell 1 ($startRow + 1)) (Cell ($columns - 1) ($endRow + 1))
    Key 67 @(17)
    $copyExpected = ('COPY-PROSE ' + ('alpha beta gamma ' * 18)).TrimEnd() + "`n`n" + '```ts' + "`n    const value = 1;`n" + '```'
    [void](Check 'copyOmitsPaddingAndSoftWraps' { [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $copyExpected, [StringComparison]::Ordinal) -and -not (State).selectionActive })
    Screenshot 'copy-prose'
    Fixture-Command 'copy-editor'
    $copyFrame = State
    $last = 'caf' + [char]0xE9 + ' ' + [char]0x754C
    $startRow = @(0..($copyFrame.painted.Count - 1) | Where-Object { $copyFrame.painted[$_].Contains('COPY-EDITOR') })[0]
    $endRow = @(0..($copyFrame.painted.Count - 1) | Where-Object { $copyFrame.painted[$_].Trim() -ceq $last })[-1]
    if ($null -eq $startRow -or $null -eq $endRow -or $endRow -le $startRow) { throw 'Wrapped editor draft is not fully visible' }
    Drag (Cell 1 ($startRow + 1)) (Cell ($columns - 1) ($endRow + 1))
    Key 67 @(17)
    $copyExpected = ('COPY-EDITOR ' + ('alpha beta gamma ' * 12)).TrimEnd() + "`n`n    " + $last
    [void](Check 'editorCopyOmitsSoftWraps' { [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), $copyExpected, [StringComparison]::Ordinal) -and -not (State).selectionActive })
    foreach ($reverse in @($false, $true)) {
        Fixture-Command 'copy-seam'
        $seamFrame = State
        $startRow = @(0..($seamFrame.painted.Count - 1) | Where-Object { $seamFrame.painted[$_].Trim() -ceq 'SEAM-ONE' })[0]
        $endRow = @(0..($seamFrame.painted.Count - 1) | Where-Object { $seamFrame.painted[$_].Trim() -ceq 'DRAFT-SEAM' })[0]
        if ($null -eq $startRow -or $null -eq $endRow) { throw 'Seam fixture is not fully visible' }
        [System.Windows.Forms.Clipboard]::SetText('SEAM-SENTINEL')
        $copyCount = (State).keyCopy
        if ($reverse) { Drag (Cell 11 ($endRow + 1)) (Cell 1 ($startRow + 1)) }
        else { Drag (Cell 1 ($startRow + 1)) (Cell 11 ($endRow + 1)) }
        if (-not (Wait-For { (State).selectionPainted -and (State).frameFlushed })) { throw 'Cross-seam selection was not painted' }
        Key 67 @(17)
        $name = if ($reverse) { 'selectionCrossesIntoTranscript' } else { 'selectionCrossesIntoEditor' }
        [void](Check $name { (State).keyCopy -eq ($copyCount + 1) -and [String]::Equals([System.Windows.Forms.Clipboard]::GetText(), "SEAM-ONE`nSEAM-TWO`n`nDRAFT-SEAM", [StringComparison]::Ordinal) -and -not (State).selectionActive })
    }
    Screenshot 'copy-seam'
    Key 81 @(17)
    [void](Check 'orderlyExit' { (State).stopped -eq $true -and (Test-Path (Join-Path $OutputDirectory 'stty-after.txt')) })
    [void](Check 'termiosRestored' { (State).termiosBefore -and (State).termiosBefore -ceq (State).termiosAfter })
    Screenshot 'restored'
} catch {
    $report.error = $_.ToString()
    if ($handle -ne [IntPtr]::Zero -and [ProbeDesktop]::GetForegroundWindow() -eq $handle) { Screenshot 'failure' }
} finally {
    Cleanup-OwnedResources
    $report.passed = $report.checks.Count -eq $requiredChecks.Count -and @($requiredChecks | Where-Object { -not $report.checks.Contains($_) }).Count -eq 0 -and @($report.checks.Values | Where-Object { -not $_.passed }).Count -eq 0 -and -not $report.Contains('error') -and -not $report.Contains('cleanupError')
    [System.IO.File]::WriteAllText((Join-Path $OutputDirectory 'report.json'), ($report | ConvertTo-Json -Depth 10))
    $report | ConvertTo-Json -Depth 10
}
if (-not $report.passed) { exit 1 }

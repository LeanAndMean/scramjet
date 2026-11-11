# Installs Scramjet 2 on Windows from its newest GitHub release, prereleases included:
#   irm https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.ps1 | iex
# SCRAMJET_INSTALL_DIR names the directory (default %LOCALAPPDATA%\Programs\scramjet);
# SCRAMJET_VERSION a release (2.143.1) instead of the newest. The archive is checked against the
# release's SHA256SUMS, and the program must run and say its version before it replaces anything.
# The directory is put first on your user Path, ahead of Scramjet 1's npm directory, unless
# SCRAMJET_NO_MODIFY_PATH is set.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'LeanAndMean/scramjet'
$dir = if ($env:SCRAMJET_INSTALL_DIR) { $env:SCRAMJET_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\scramjet' }

function Fail($message) {
    throw "install.ps1: $message"
}

$machine = [Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE', 'Machine')
if ($machine -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64' -and $machine -ne 'ARM64') {
    Fail "no release for $machine Windows"
}
# On ARM64 Windows the x86_64 build runs under its emulation.

if ($env:SCRAMJET_VERSION) {
    $version = $env:SCRAMJET_VERSION -replace '^v', ''
} else {
    $listed = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$repo/releases?per_page=1"
    $tag = @($listed.tag_name)[0]
    if (-not $tag) { Fail "$repo has no release" }
    $version = $tag -replace '^v', ''
}

$name = "scramjet-$version-x86_64-windows.tar.gz"
$base = "https://github.com/$repo/releases/download/v$version"
$work = Join-Path ([IO.Path]::GetTempPath()) ("scramjet-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
try {
    Write-Host "Downloading $name"
    try {
        Invoke-WebRequest -UseBasicParsing "$base/$name" -OutFile (Join-Path $work $name)
        Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile (Join-Path $work 'SHA256SUMS')
    } catch {
        Fail "could not download from ${base}: $($_.Exception.Message)"
    }

    $expected = $null
    foreach ($line in Get-Content (Join-Path $work 'SHA256SUMS')) {
        $hash, $file = $line -split '\s+', 2
        if ($file -and $file.TrimStart('*') -eq $name) { $expected = $hash.ToLower() }
    }
    if (-not $expected) { Fail "SHA256SUMS does not list $name" }
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $work $name)).Hash.ToLower()
    if ($actual -ne $expected) { Fail "$name does not match its checksum in SHA256SUMS; nothing was installed" }

    & "$env:SystemRoot\System32\tar.exe" -xzf (Join-Path $work $name) -C $work scramjet.exe
    if ($LASTEXITCODE -ne 0) { Fail "could not unpack $name" }
    $new = Join-Path $work 'scramjet.exe'
    try { $said = (& $new --version | Out-String).Trim() } catch { Fail "the downloaded program does not run here: $($_.Exception.Message)" }
    if ($LASTEXITCODE -ne 0) { Fail "the downloaded program does not run here (exit code $LASTEXITCODE)" }

    New-Item -ItemType Directory -Force $dir | Out-Null
    $target = Join-Path $dir 'scramjet.exe'
    # A running scramjet.exe cannot be overwritten, but it can be renamed out of the way.
    $old = "$target.old"
    if (Test-Path $old) { Remove-Item -Force $old -ErrorAction SilentlyContinue }
    if (Test-Path $target) { Move-Item -Force $target $old }
    Move-Item $new $target
    Write-Host "Installed Scramjet $said as $target"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

# The user Path with $dir first, or $null when it is first already.
function First-On-Path([string]$userPath, [string]$dir) {
    $same = { [Environment]::ExpandEnvironmentVariables($args[0]).TrimEnd('\') -eq $dir.TrimEnd('\') }
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries.Count -gt 0 -and (& $same $entries[0])) { return $null }
    $rest = @($entries | Where-Object { -not (& $same $_) })
    return (@($dir) + $rest) -join ';'
}

$userPath = (Get-Item 'HKCU:\Environment').GetValue('Path', '', 'DoNotExpandEnvironmentNames')
$value = First-On-Path $userPath $dir
if ($null -eq $value) {
    # Already first.
} elseif ($env:SCRAMJET_NO_MODIFY_PATH) {
    Write-Host "$dir is not first on your user Path; put it in front to run this scramjet."
} else {
    # Written raw, so entries such as %USERPROFILE%\bin keep their variables.
    Set-ItemProperty 'HKCU:\Environment' -Name Path -Value $value -Type ExpandString
    # Setting a variable through .NET tells running programs that the environment changed.
    [Environment]::SetEnvironmentVariable('SCRAMJET_INSTALL_NOTIFY', '1', 'User')
    [Environment]::SetEnvironmentVariable('SCRAMJET_INSTALL_NOTIFY', $null, 'User')
    Write-Host "Put $dir first on your user Path; open a new terminal to use it."
}
$env:Path = "$dir;$env:Path"

$others = @(Get-Command scramjet -All -ErrorAction SilentlyContinue | Where-Object { $_.Source -and $_.Source -ne $target })
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
foreach ($other in $others) {
    $otherDir = Split-Path $other.Source
    if (($machinePath -split ';') | Where-Object { $_.TrimEnd('\') -eq $otherDir.TrimEnd('\') }) {
        Write-Host "$($other.Source) is on the system Path, which comes before the user's: it runs first."
    }
}
Write-Host 'Later releases: scramjet update self'

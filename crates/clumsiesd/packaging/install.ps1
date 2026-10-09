# User-level atomic binary-pair replacement. No credentials or daemon data are removed.
param(
    [string]$Source = $PSScriptRoot,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'Programs\ClumsiesCLI'),
    [switch]$Uninstall,
    [switch]$AddToPath
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $InstallRoot | Out-Null
$lock = $null
$stage = Join-Path $InstallRoot ('.stage-' + [guid]::NewGuid().ToString('N'))
$runtime = Join-Path $InstallRoot 'runtime'
$backup = Join-Path $InstallRoot '.previous'
$switched = $false
# Probe every executable before removing any file, including during uninstall.
function AssertRuntimeUnused {
    if (Test-Path $runtime) {
        foreach ($name in @('clumsies.exe', 'clumsiesd.exe')) {
            try { $probe = [IO.File]::Open((Join-Path $runtime $name), 'Open', 'ReadWrite', 'None'); $probe.Dispose() }
            catch { throw "Close active CLI/MCP processes before changing programs; $name is still in use" }
        }
    }
}
try {
    $lock = [IO.File]::Open((Join-Path $InstallRoot '.install.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
    if ((Test-Path $runtime) -and -not (Test-Path "$runtime\.clumsies-cli")) { throw "Refusing to replace an unrelated directory: $runtime" }
    if ((Test-Path $runtime) -and ((Get-Item $runtime).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Refusing a linked runtime directory' }
    if ($Uninstall) {
        if (Test-Path "$runtime\clumsies.exe") {
            & "$runtime\clumsies.exe" daemon stop
            if ($LASTEXITCODE -ne 0) { throw 'Daemon could not stop; programs were retained' }
        }
        AssertRuntimeUnused
        if (Test-Path $runtime) { Remove-Item -Recurse -Force $runtime }
        $path = [string][Environment]::GetEnvironmentVariable('Path', 'User')
        [Environment]::SetEnvironmentVariable('Path', (($path -split ';' | Where-Object { $_ -and $_ -ne $runtime }) -join ';'), 'User')
        Write-Output 'Removed programs; credentials, bindings, cache, and local drafts were retained.'
        return
    }
    $verified = @()
    foreach ($line in Get-Content (Join-Path $Source 'SHA256SUMS')) {
        if ($line -notmatch '^([0-9a-f]{64})  ([A-Za-z0-9_.-]+)$') { throw 'Invalid checksum manifest' }
        $expected = $Matches[1]; $name = $Matches[2]
        if ($name -in $verified) { throw "Duplicate checksum entry: $name" }
        $verified += $name
        if ((Get-FileHash (Join-Path $Source $name) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw "Checksum mismatch: $name" }
    }
    foreach ($name in @('clumsies.exe', 'clumsiesd.exe', 'install.ps1', '.clumsies-cli')) {
        if ($name -notin $verified) { throw "Checksum manifest omits $name" }
    }
    foreach ($name in @('clumsies.exe', 'clumsiesd.exe')) {
        if (-not (Test-Path (Join-Path $Source $name))) { throw "Missing executable $name" }
    }
    if (Test-Path $backup) { throw "Recovery directory already exists: $backup" }
    New-Item -ItemType Directory $stage | Out-Null
    Copy-Item "$Source\*" $stage -Recurse
    & "$stage\clumsies.exe" --version
    if ($LASTEXITCODE -ne 0) { throw 'Staged executable does not run; check runtime dependencies' }
    $stopper = if (Test-Path "$runtime\clumsies.exe") { "$runtime\clumsies.exe" } else { "$stage\clumsies.exe" }
    & $stopper daemon stop
    if ($LASTEXITCODE -ne 0) { throw 'Daemon could not stop; installation was not changed' }
    AssertRuntimeUnused
    if (Test-Path $runtime) { Move-Item $runtime $backup }
    $switched = $true
    Move-Item $stage $runtime
    if ($AddToPath) {
        $path = [string][Environment]::GetEnvironmentVariable('Path', 'User')
        if ($runtime -notin ($path -split ';')) { [Environment]::SetEnvironmentVariable('Path', (($path.TrimEnd(';') + ';' + $runtime).TrimStart(';')), 'User') }
    }
    $switched = $false
    if (Test-Path $backup) {
        try { Remove-Item -Recurse -Force $backup }
        catch { Write-Warning "Installed programs are ready; old programs remain at $backup. Close old processes and remove that recovery directory before the next upgrade." }
    }
    Write-Output "Installed $runtime. Reopen terminals and reconnect Agent hosts after upgrading."
} catch {
    if ($switched) {
        if (Test-Path $runtime) { Remove-Item -Recurse -Force $runtime }
        if (Test-Path $backup) { Move-Item $backup $runtime }
    }
    throw
} finally {
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    if ($lock) { $lock.Dispose() }
}

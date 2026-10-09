# Exercise the installer's actual barrier with a delayed release, persistent lock, and missing image.
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'install.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw "Installer parse errors: $errors" }
$definition = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'AssertRuntimeUnused' }, $true)
if (-not $definition) { throw 'Missing installer executable barrier' }
. ([scriptblock]::Create($definition.Extent.Text))
$root = Join-Path $env:RUNNER_TEMP ('cli-release-barrier-' + [guid]::NewGuid().ToString('N'))
$runtime = Join-Path $root 'runtime'
$ready = Join-Path $root 'ready'
$release = Join-Path $root 'release'
$child = $null; $held = $null
$script:waits = 0; $script:releaseOnWait = $true
# Release only after the production guard actually encounters the held image.
function Start-Sleep {
    param([int]$Milliseconds)
    $script:waits++
    if ($script:releaseOnWait) { Set-Content $release 'release' }
    Microsoft.PowerShell.Utility\Start-Sleep -Milliseconds $Milliseconds
}
try {
    New-Item -ItemType Directory -Force $runtime | Out-Null
    Set-Content "$runtime\clumsies.exe" 'client'
    Set-Content "$runtime\clumsiesd.exe" 'resident'
    $holder = Join-Path $root 'holder.ps1'
    @'
param($Runtime, $Ready, $Release)
$stream = [IO.File]::Open((Join-Path $Runtime 'clumsiesd.exe'), 'Open', 'Read', 'Read')
try {
    Set-Content $Ready 'ready'
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path $Release)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Release signal timed out' }
        Start-Sleep -Milliseconds 25
    }
} finally { $stream.Dispose() }
'@ | Set-Content $holder
    $child = Start-Process (Join-Path $PSHOME 'pwsh.exe') -ArgumentList @('-NoProfile', '-File', "`"$holder`"", "`"$runtime`"", "`"$ready`"", "`"$release`"") -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path $ready)) {
        if ($child.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'Holder did not become ready' }
        Microsoft.PowerShell.Utility\Start-Sleep -Milliseconds 25
    }
    AssertRuntimeUnused
    if ($script:waits -eq 0) { throw 'Transient-lock path was not exercised' }
    if (-not $child.WaitForExit(5000)) { throw 'Holder did not exit' }
    $child.Refresh()
    if ($child.ExitCode -ne 0) { throw 'Holder failed to exit' }
    $script:releaseOnWait = $false
    $held = [IO.File]::Open("$runtime\clumsiesd.exe", 'Open', 'Read', 'Read')
    $failed = $false
    try { AssertRuntimeUnused } catch {
        if ($_.Exception.Message -notlike '*still in use*') { throw }
        $failed = $true
    }
    if (-not $failed -or -not (Test-Path "$runtime\clumsies.exe")) { throw 'Persistent lock was not safely rejected' }
    $held.Dispose(); $held = $null
    Remove-Item "$runtime\clumsiesd.exe"
    $waits = $script:waits; $failed = $false
    try { AssertRuntimeUnused } catch { $failed = $true }
    if (-not $failed -or $script:waits -ne $waits) { throw 'Non-sharing failure was retried' }
    Write-Output 'Transient release, persistent-lock timeout, and non-sharing failure checks passed.'
} finally {
    if ($held) { $held.Dispose() }
    if ($child -and -not $child.HasExited) { $child.Kill(); $child.WaitForExit() }
    if (Test-Path $root) { Remove-Item -Recurse -Force $root }
}

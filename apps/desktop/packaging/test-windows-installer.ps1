# Run only on a disposable Windows CI runner: installation creates HKCU entries
# and user shortcuts. The daemon state is isolated under RUNNER_TEMP.
[CmdletBinding()]
param([string]$Out = 'dist')
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) {
    throw 'Installer lifecycle checks require a disposable GitHub Actions runner'
}
$installer = (Get-ChildItem $Out -Filter '*-Setup.exe').FullName
if (@($installer).Count -ne 1) { throw 'Expected one installer' }
$install = Join-Path $env:LOCALAPPDATA 'Programs/Clumsies'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Clumsies.lnk'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Clumsies.lnk'
$registry = 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Uninstall/ai.clumsies.desktop_is1'
if ((Test-Path $install) -or (Test-Path $registry) -or (Test-Path $shortcut) -or (Test-Path $desktopShortcut)) {
    throw 'Refusing to change an existing installation'
}
$env:CLUMSIES_DAEMON_ROOT = Join-Path $env:RUNNER_TEMP 'clumsies-installer-state'
New-Item -ItemType Directory -Path $env:CLUMSIES_DAEMON_ROOT | Out-Null
$marker = Join-Path $env:CLUMSIES_DAEMON_ROOT 'keep-user-data.txt'
Set-Content $marker 'preserve this data'

function Run-Setup {
    $process = Start-Process $installer -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/RESTARTEXITCODE=9', '/TASKS=desktopicon' -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
}
function Check-Installed {
    if (-not (Test-Path $shortcut) -or -not (Test-Path $desktopShortcut) -or -not (Test-Path $registry)) {
        throw 'Installer did not create shortcuts and uninstall registration'
    }
    $shell = New-Object -ComObject WScript.Shell
    if ($shell.CreateShortcut($shortcut).TargetPath -ne (Join-Path $install 'clumsies-desktop.exe')) {
        throw 'Start menu shortcut has the wrong target'
    }
    foreach ($program in @('clumsies-desktop.exe', 'clumsiesd.exe')) {
        $staged = (Get-ChildItem $Out -Directory -Filter 'Clumsies-*-windows-x86_64').FullName
        if ((Get-FileHash (Join-Path $install $program)).Hash -ne (Get-FileHash (Join-Path $staged $program)).Hash) {
            throw "Installed $program does not match the package"
        }
    }
    & (Join-Path $install 'clumsies-desktop.exe') --smoke-test
    if ($LASTEXITCODE -ne 0) { throw 'Installed daemon startup and IPC failed' }
}
try {
    Run-Setup
    Check-Installed
    # Reinstall while the resident daemon holds its executable open.
    Set-Content (Join-Path $install 'README.txt') 'old installation'
    Run-Setup
    Check-Installed
    if ((Get-Content (Join-Path $install 'README.txt') -Raw).Trim() -eq 'old installation') {
        throw 'Reinstall did not replace existing files'
    }
    # A long-running stand-in for the GUI checks that Setup never kills it.
    $client = Join-Path $install 'clumsies-desktop.exe'
    Copy-Item "$env:WINDIR/System32/ping.exe" $client -Force
    $runningClient = Start-Process $client -ArgumentList '-t', '127.0.0.1' -PassThru -WindowStyle Hidden
    try {
        Start-Sleep -Milliseconds 500
        $blocked = Start-Process $installer -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
        if ($blocked.ExitCode -ne 7 -or $runningClient.HasExited) { throw 'Setup must refuse to replace a running client' }
        $blockedUninstall = Start-Process (Join-Path $install 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
        if ($blockedUninstall.ExitCode -eq 0 -or $runningClient.HasExited) { throw 'Uninstall must refuse to remove a running client' }
    } finally {
        if (-not $runningClient.HasExited) { $runningClient.Kill(); $runningClient.WaitForExit() }
    }
    Run-Setup
    Check-Installed
    $uninstall = Start-Process (Join-Path $install 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
    if ($uninstall.ExitCode -ne 0) { throw "Uninstall failed: $($uninstall.ExitCode)" }
    if ((Test-Path $client) -or (Test-Path $registry) -or (Test-Path $shortcut) -or (Test-Path $desktopShortcut)) {
        throw 'Uninstall left program files, registration or shortcuts'
    }
    if ((Get-Content $marker) -ne 'preserve this data') { throw 'Uninstall changed user data' }
    Write-Output 'Installer lifecycle passed: install, shortcuts, IPC, reinstall, running-client guard, uninstall, data preservation'
} finally {
    Get-Process clumsiesd -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $install 'clumsiesd.exe') } | Stop-Process
}

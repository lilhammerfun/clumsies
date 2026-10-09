# Validate portable install, locked upgrades, installer reinstall, and retained user data.
param([Parameter(Mandatory)][string]$Package, [Parameter(Mandatory)][string]$Installer)
$ErrorActionPreference = 'Stop'
$testRoot = Join-Path $env:RUNNER_TEMP ('clumsies-cli-' + [guid]::NewGuid().ToString('N'))
$programs = Join-Path $testRoot 'programs'
$runtime = Join-Path $programs 'runtime'
$data = Join-Path $testRoot 'data'
$originalPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$env:CLUMSIES_DAEMON_ROOT = $data
$env:CLUMSIES_DAEMON_CACHE_DIR = Join-Path $testRoot 'cache'
$env:CLUMSIES_DAEMON_LOG_DIR = Join-Path $testRoot 'logs'
$env:CLUMSIES_SYNC_ENABLED = 'false'
Remove-Item Env:CLUMSIES_DEV_INSTANCE_ID -ErrorAction SilentlyContinue
Remove-Item Env:CLUMSIES_AGENT_RUNTIME_TEST_MACH_SERVICE -ErrorAction SilentlyContinue
function RunCli([string[]]$Arguments) {
    $result = & "$runtime\clumsies.exe" @Arguments
    if ($LASTEXITCODE -ne 0) { throw "CLI failed: $Arguments" }
    return $result
}
# Bound GUI processes and retain installer diagnostics when silent execution stalls.
function RunSetupProcess([string]$Program, [string[]]$Parameters) {
    $log = Join-Path $testRoot ([guid]::NewGuid().ToString('N') + '.setup.log')
    $process = Start-Process $Program -ArgumentList ($Parameters + "/LOG=`"$log`"") -PassThru
    if (-not $process.WaitForExit(120000)) {
        $process.Kill($true)
        if (Test-Path $log) { Get-Content $log -Tail 60 | Write-Output }
        throw "Installer process timed out: $Program"
    }
    $process.Refresh()
    return $process
}
function RunInstaller {
    $process = RunSetupProcess $Installer @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$programs`"")
    if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
}
try {
    New-Item -ItemType Directory -Force $testRoot, $data | Out-Null
    $source = Join-Path $testRoot 'package'
    Expand-Archive $Package $source
    & "$source\install.ps1" -Source $source -InstallRoot $programs
    $started = (RunCli -Arguments @('daemon', 'start')) | ConvertFrom-Json
    $reused = (RunCli -Arguments @('daemon', 'start')) | ConvertFrom-Json
    if ($started.daemon_installation_id -ne $reused.daemon_installation_id) { throw 'Resident was not reused' }
    Set-Content (Join-Path $data 'retained-work') 'Retained draft and binding proof'
    & "$source\install.ps1" -Source $source -InstallRoot $programs
    $upgraded = (RunCli -Arguments @('daemon', 'start')) | ConvertFrom-Json
    if ($started.daemon_installation_id -ne $upgraded.daemon_installation_id) { throw 'Upgrade changed installation identity' }
    RunCli -Arguments @('daemon', 'stop') | Out-Null
    $before = (Get-FileHash "$runtime\clumsiesd.exe").Hash
    $held = [IO.File]::Open("$runtime\clumsiesd.exe", 'Open', 'Read', 'Read')
    try {
        $failed = $false
        try { & "$source\install.ps1" -Source $source -InstallRoot $programs } catch { $failed = $true }
        if (-not $failed) { throw 'Upgrade replaced an active MCP executable' }
        $failed = $false
        try { & "$source\install.ps1" -Uninstall -InstallRoot $programs } catch { $failed = $true }
        if (-not $failed -or -not (Test-Path "$runtime\clumsies.exe")) { throw 'Uninstall partially removed active programs' }
    } finally { $held.Dispose() }
    if ((Get-FileHash "$runtime\clumsiesd.exe").Hash -ne $before) { throw 'Failed upgrade changed the resident' }
    Add-Content "$source\clumsiesd.exe" 'corruption'
    $failed = $false
    try { & "$source\install.ps1" -Source $source -InstallRoot $programs } catch { $failed = $true }
    if (-not $failed) { throw 'Corrupt package unexpectedly installed' }
    RunInstaller
    RunInstaller
    $installed = (RunCli -Arguments @('daemon', 'start')) | ConvertFrom-Json
    if ($started.daemon_installation_id -ne $installed.daemon_installation_id) { throw 'Installer lost retained data' }
    RunCli -Arguments @('daemon', 'stop') | Out-Null
    $uninstaller = Get-ChildItem "$programs\unins*.exe" | Select-Object -First 1
    $held = [IO.File]::Open("$runtime\clumsiesd.exe", 'Open', 'Read', 'Read')
    try {
        $blocked = RunSetupProcess $uninstaller.FullName @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
        if ($blocked.ExitCode -eq 0 -or -not (Test-Path "$runtime\clumsies.exe") -or -not (Test-Path "$programs\install.ps1")) { throw 'Installer uninstall partially removed programs while MCP was active' }
    } finally { $held.Dispose() }
    $process = RunSetupProcess $uninstaller.FullName @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
    if ($process.ExitCode -ne 0) { throw 'Uninstall failed' }
    if (Test-Path "$runtime\clumsies.exe") { throw 'Uninstall retained the program' }
    if (-not (Test-Path "$data\local.db") -or -not (Test-Path "$data\retained-work")) { throw 'Uninstall lost user data' }
    Write-Output 'Windows portable/install/reinstall/upgrade/active-process/corruption/uninstall checks passed.'
} finally {
    if (Test-Path "$runtime\clumsies.exe") { & "$runtime\clumsies.exe" daemon stop }
    [Environment]::SetEnvironmentVariable('Path', $originalPath, 'User')
    if (Test-Path $testRoot) { Remove-Item -Recurse -Force $testRoot }
}

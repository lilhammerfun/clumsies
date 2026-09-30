# Assemble the Windows package: the client, the engine it starts, the README and
# the mark, as an installer and a portable zip with checksums beside them.
#
#   pwsh -File assemble-windows.ps1 <version> <binary-dir> <out-dir>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)][string]$Version,
    [Parameter(Mandatory = $true, Position = 1)][string]$Binaries,
    [Parameter(Mandatory = $true, Position = 2)][string]$Out
)

$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Expected a stable product version (X.Y.Z)' }
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$package = "Clumsies-$Version-windows-x86_64"

foreach ($program in @('clumsies-desktop.exe', 'clumsiesd.exe')) {
    $path = Join-Path $Binaries $program
    if (-not (Test-Path $path)) { throw "assemble-windows.ps1: $path is missing" }
}

$stage = Join-Path $Out $package
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'icons') | Out-Null

Copy-Item (Join-Path $Binaries 'clumsies-desktop.exe') $stage
Copy-Item (Join-Path $Binaries 'clumsiesd.exe') $stage
Copy-Item (Join-Path $here 'README.txt') $stage
Copy-Item (Join-Path $here '../assets/icons/clumsies-256.png') (Join-Path $stage 'icons')
Copy-Item (Join-Path $here '../assets/icons/clumsies.ico') (Join-Path $stage 'icons')

$archive = Join-Path $Out "$package.zip"
if (Test-Path $archive) { Remove-Item -Force $archive }
Compress-Archive -Path $stage -DestinationPath $archive

$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
"$hash  $package.zip" | Out-File -Encoding ascii "$archive.sha256"
Write-Output "assembled $archive"

$compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
if (-not (Test-Path $compiler)) { throw 'Inno Setup 6 is required to build the Windows installer' }
& $compiler "/DAppVersion=$Version" "/DPackageDir=$((Resolve-Path $stage).Path)" "/O$((Resolve-Path $Out).Path)" (Join-Path $here 'windows.iss')
if ($LASTEXITCODE -ne 0) { throw 'Windows installer compilation failed' }
$installer = Join-Path $Out "$package-Setup.exe"
$hash = (Get-FileHash $installer -Algorithm SHA256).Hash.ToLower()
"$hash  $package-Setup.exe" | Out-File -Encoding ascii "$installer.sha256"
Write-Output "assembled $installer"
Get-ChildItem $Out | Select-Object Name, Length

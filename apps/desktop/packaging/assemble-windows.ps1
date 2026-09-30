# Assemble the Windows package: the client, the engine it starts, the README and
# the mark, in a zip with a checksum beside it.
#
#   pwsh -File assemble-windows.ps1 <version> <binary-dir> <out-dir>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)][string]$Version,
    [Parameter(Mandatory = $true, Position = 1)][string]$Binaries,
    [Parameter(Mandatory = $true, Position = 2)][string]$Out
)

$ErrorActionPreference = 'Stop'
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
Get-ChildItem $Out | Select-Object Name, Length

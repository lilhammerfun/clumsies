# Package a matching CLI/resident pair, private installer, and MSVC runtime dependencies.
param(
    [Parameter(Mandatory, Position=0)][string]$Version,
    [Parameter(Mandatory, Position=1)][string]$Binaries,
    [Parameter(Mandatory, Position=2)][string]$Out
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Expected version X.Y.Z' }
$package = "clumsies-cli-$Version-windows-x86_64"
$stage = Join-Path $Out $package
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($name in @('clumsies.exe', 'clumsiesd.exe')) { Copy-Item (Join-Path $Binaries $name) $stage }
Copy-Item (Join-Path $PSScriptRoot 'install.ps1') $stage
[IO.File]::WriteAllText((Join-Path $stage '.clumsies-cli'), "$Version`n")
$reported = & "$stage\clumsies.exe" --version
if ($LASTEXITCODE -ne 0 -or $reported -ne "clumsies $Version") { throw 'CLI and package versions differ' }
$reported = & "$stage\clumsiesd.exe" --version
if ($LASTEXITCODE -ne 0 -or $reported -ne "clumsiesd $Version") { throw 'Daemon and package versions differ' }
# Use the same licensed app-local CRT deployment as the existing desktop packaging.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $visualStudio) { throw 'Visual Studio C++ redistributables are required' }
$crt = Get-ChildItem "$visualStudio/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT" -Directory | Sort-Object { [version]$_.Parent.Parent.Name } -Descending | Select-Object -First 1
if (-not $crt) { throw 'Missing x64 MSVC redistributables' }
Copy-Item (Join-Path $crt.FullName '*.dll') $stage
foreach ($library in @('msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
    if (-not (Test-Path (Join-Path $stage $library))) { throw "Missing runtime dependency: $library" }
}
$checksums = Get-ChildItem -Force $stage -File | Sort-Object Name | ForEach-Object { (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $_.Name }
$checksums | Set-Content -Encoding ascii (Join-Path $stage 'SHA256SUMS')
$archive = Join-Path $Out "$package.zip"
if (Test-Path $archive) { Remove-Item $archive }
# Compress-Archive omits hidden dot files; ZipFile retains the package marker.
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory((Resolve-Path $stage).Path, [IO.Path]::GetFullPath($archive))
((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() + "  $package.zip") | Set-Content -Encoding ascii "$archive.sha256"
$compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
if (-not (Test-Path $compiler)) { throw 'Inno Setup 6 is required' }
& $compiler "/DAppVersion=$Version" "/DPackageDir=$((Resolve-Path $stage).Path)" "/O$((Resolve-Path $Out).Path)" (Join-Path $PSScriptRoot 'windows.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
$installer = Join-Path $Out "$package-Setup.exe"
((Get-FileHash $installer -Algorithm SHA256).Hash.ToLowerInvariant() + "  $package-Setup.exe") | Set-Content -Encoding ascii "$installer.sha256"

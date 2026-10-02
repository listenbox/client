param(
    [Parameter(Mandatory = $true)][ValidateSet('x64', 'arm64')][string] $Architecture,
    [Parameter(Mandatory = $true)][string] $Payload
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Set-Location (Split-Path $PSScriptRoot -Parent)
$Payload = (Resolve-Path $Payload).Path
$release = & node tools/release.mjs validate
if ($LASTEXITCODE -ne 0) { throw 'Invalid release identity.' }
$version = ($release | ConvertFrom-Json).version
if (-not (Test-Path (Join-Path $Payload 'listenbox-desktop.exe')) -or -not (Test-Path (Join-Path $Payload 'WinSparkle.dll'))) {
    throw 'The installer requires the desktop application and WinSparkle runtime.'
}
$compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
if (-not (Test-Path $compiler)) { throw 'Install the pinned Inno Setup 6.7.3 compiler.' }
& $compiler "/DAppVersion=$version" "/DArchitecture=$Architecture" "/DPayload=$Payload" tools/windows-installer.iss
if ($LASTEXITCODE -ne 0) { throw 'Windows installer compilation failed.' }

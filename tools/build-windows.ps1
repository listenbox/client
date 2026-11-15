param(
    [Parameter(Mandatory = $true)][ValidateSet('x64', 'arm64')][string] $Architecture
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT') { throw 'This build requires Windows.' }
Set-Location (Split-Path $PSScriptRoot -Parent)

function Invoke-Checked {
    param([string] $Program, [string[]] $Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
$goArchitecture = if ($Architecture -eq 'arm64') { 'arm64' } else { 'amd64' }
$actual = & go env GOARCH
if ($LASTEXITCODE -ne 0 -or $actual -ne $goArchitecture) { throw "Use the native $Architecture Go and MSYS2 SDK on this runner." }
if (-not $env:MSYS2_LOCATION) { throw 'Install MSYS2 and set MSYS2_LOCATION before installing the scoped Mise tools.' }
$flavor = if ($Architecture -eq 'arm64') { 'clangarm64' } else { 'clang64' }
$sdk = Join-Path $env:MSYS2_LOCATION "$flavor/bin"
$env:PATH = "$sdk;$(Join-Path $env:MSYS2_LOCATION 'usr/bin');$env:PATH"
$env:PKG_CONFIG = Join-Path $sdk 'pkg-config.exe'
$env:CGO_ENABLED = '1'
$version = (Get-Content VERSION -Raw).Trim()
$commit = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source commit.' }
$key = $env:LISTENBOX_UPDATE_PUBLIC_KEY
if ($env:LISTENBOX_PRODUCTION_RELEASE -eq '1' -and -not $key) { throw 'A production release requires the existing WinSparkle Ed25519 key.' }

Invoke-Checked go @('test', '-count=1', '-timeout=30s', './...')
# MyGo supplies the icon, version resources, manifest and GUI subsystem.
# Keep the Inno installer/AppId for existing WinSparkle installations.
$env:MYGO_LDFLAGS = "-extldflags=-static -X github.com/listenbox/client/crates/sync-engine.Version=$version -X github.com/listenbox/client/crates/sync-engine.SourceCommit=$commit -X github.com/listenbox/client/crates/sync-engine.Production=1 -X main.updatePublicKey=$key -X main.updatePlatform=windows-$Architecture"
Invoke-Checked go @('tool', 'mygo', 'build', '-platform', "windows/$goArchitecture", '-skip-installer', 'crates/desktop')
$package = "crates/desktop/dist/windows-$Architecture"
New-Item -ItemType Directory -Force $package | Out-Null
Copy-Item "crates/desktop/dist/mygo/windows-$goArchitecture/*" $package -Recurse -Force
$executable = Join-Path $package 'listenbox-desktop.exe'

# Inspect the actual shipped PE files, including GUI subsystem and dependencies.
$readobj = Join-Path $sdk 'llvm-readobj.exe'
$expectedMachine = if ($Architecture -eq 'arm64') { 'IMAGE_FILE_MACHINE_ARM64' } else { 'IMAGE_FILE_MACHINE_AMD64' }
foreach ($file in @($executable, (Join-Path $package 'WinSparkle.dll'))) {
    $headers = & $readobj --file-headers $file
    if ($LASTEXITCODE -ne 0 -or -not ($headers -match $expectedMachine)) { throw "Wrong PE architecture: $file" }
}
$headers = & $readobj --file-headers $executable
if (-not ($headers -match 'IMAGE_SUBSYSTEM_WINDOWS_GUI')) { throw 'The desktop executable must use the Windows GUI subsystem.' }
$imports = & $readobj --coff-imports $executable
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect the shipped PE dependencies.' }
$imports | Write-Output
if ($imports -match '(?i)(VCRUNTIME|MSVCP|ucrtbased|libc\+\+|libunwind|libgcc|libwinpthread|avcodec|avformat|avutil|swresample).*\.dll') { throw 'A separately distributed compiler or codec runtime DLL is required.' }
$startup = Start-Process -FilePath $executable -ArgumentList '--version' -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput '.cache/windows-version.txt'
if ($startup.ExitCode -ne 0 -or (Get-Content '.cache/windows-version.txt' -Raw).Trim() -ne "Listenbox $version ($commit)") { throw 'Packaged desktop version/source or startup is invalid.' }

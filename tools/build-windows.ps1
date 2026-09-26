param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('x64', 'arm64')]
    [string] $Architecture,
    [Parameter(Mandatory = $true)]
    [ValidateSet('FFmpeg', 'Release')]
    [string] $Stage
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT') { throw 'This build requires Windows.' }

function Invoke-Checked {
    param([string] $Program, [string[]] $Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}

$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$platform = switch ($Architecture) {
    'x64' { @{ Target = 'x86_64-pc-windows-msvc'; Compiler = 'amd64'; Component = 'x86.x64'; Machine = '8664 machine \(x64\)' } }
    'arm64' { @{ Target = 'aarch64-pc-windows-msvc'; Compiler = 'arm64'; Component = 'ARM64'; Machine = 'AA64 machine \(ARM64\)' } }
}
$target = $platform.Target

if (-not $env:MSYS2_LOCATION) { throw 'Set MSYS2_LOCATION to an MSYS2 installation with make, diffutils, tar, xz, and openssl.' }
$env:PATH = "$(Join-Path $env:MSYS2_LOCATION 'usr/bin');$env:PATH"
# MSYS must retain the MSVC developer environment, rather than select MinGW.
$env:MSYS2_PATH_TYPE = 'inherit'
$env:MSYSTEM = 'MSYS'
$env:CHERE_INVOKING = '1'

if (-not $env:LIBCLANG_PATH) {
    $env:LIBCLANG_PATH = Join-Path $env:ProgramFiles 'LLVM/bin'
}
if (-not (Test-Path (Join-Path $env:LIBCLANG_PATH 'libclang.dll'))) {
    throw 'Set LIBCLANG_PATH to the directory containing the host-native LLVM libclang.dll.'
}
$env:PATH = "$env:LIBCLANG_PATH;$env:PATH"

# Initialize MSVC last so its linker precedes MSYS2's unrelated link.exe.
# Moon starts each task with a fresh environment, so both stages need this.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs = & $vswhere -latest -products '*' -requires "Microsoft.VisualStudio.Component.VC.Tools.$($platform.Component)" -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vs) { throw "Install Visual Studio C++ $Architecture build tools and the Windows SDK." }
Import-Module (Join-Path $vs 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments "-arch=$($platform.Compiler) -host_arch=$($platform.Compiler)"

$linker = (Get-Command link.exe -CommandType Application | Select-Object -First 1).Source
$expectedLinker = Join-Path $env:VCToolsInstallDir "bin/Host$Architecture/$Architecture/link.exe"
if ($linker -ne $expectedLinker) { throw "Expected the MSVC linker at $expectedLinker, but PATH selected $linker." }
Write-Output "MSVC linker: $linker"
$env:FFMPEG_DIR = Join-Path $root ".cache/ffmpeg-$target"

if ($Stage -eq 'FFmpeg') {
    New-Item -ItemType Directory -Force '.cache' | Out-Null
    $tests = ".cache/native-ffmpeg-tests-$Architecture.exe"
    Invoke-Checked rustc @('--edition', '2024', '--test', 'tools/native-ffmpeg.rs', '-o', $tests)
    Invoke-Checked $tests @('--nocapture')
    $builder = ".cache/native-ffmpeg-$Architecture.exe"
    Invoke-Checked rustc @('--edition', '2024', 'tools/native-ffmpeg.rs', '-o', $builder)
    Invoke-Checked $builder @($target)
    exit 0
}

# Statically link the MSVC runtime as well as FFmpeg, SQLite, and QuickJS so the
# downloadable executable does not need a separately installed VC runtime.
$targetKey = $target.Replace('-', '_').ToUpperInvariant()
Set-Item "Env:CARGO_TARGET_${targetKey}_RUSTFLAGS" '-C target-feature=+crt-static'
Invoke-Checked cargo @('build', '--locked', '--release', '--target', $target, '-p', 'listenbox-desktop')

$dist = Join-Path $root 'crates/desktop/dist'
$package = Join-Path $dist "windows-$Architecture"
New-Item -ItemType Directory -Force $package | Out-Null
$executable = Join-Path $package 'listenbox-desktop.exe'
Copy-Item "target/$target/release/listenbox-desktop.exe" $executable -Force
Copy-Item 'LICENSE', 'THIRD-PARTY-NOTICES.txt' $package -Force
Copy-Item (Join-Path $env:FFMPEG_DIR 'NOTICE.txt') (Join-Path $package 'FFmpeg-NOTICE.txt') -Force

# A successful compiler exit alone does not prove the package has the right
# architecture or can load on a clean Windows machine of that architecture.
$headers = & dumpbin.exe /headers $executable
if ($LASTEXITCODE -ne 0 -or -not ($headers -match $platform.Machine)) {
    throw "The packaged executable is not Windows $Architecture."
}
if (-not ($headers -match '2 subsystem \(Windows GUI\)')) {
    throw 'The desktop executable must use the Windows GUI subsystem so launching it does not open a console.'
}
$dependencies = & dumpbin.exe /dependents $executable
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect Windows DLL dependencies.' }
$dependencies | Write-Output
if ($dependencies -match '(?i)(VCRUNTIME|MSVCP|ucrtbased|avcodec|avformat|avutil|swresample).*\.dll') {
    throw 'The executable unexpectedly requires a separately distributed runtime DLL.'
}
# PowerShell does not wait for GUI executables in every invocation context.
# Wait explicitly so a loader/startup failure cannot pass the packaging check.
$startup = Start-Process -FilePath $executable -ArgumentList '--help' -WindowStyle Hidden -Wait -PassThru
if ($startup.ExitCode -ne 0) { throw "Desktop startup check failed with exit code $($startup.ExitCode)." }
Copy-Item $executable (Join-Path $dist "listenbox-desktop-windows-$Architecture.exe") -Force
Compress-Archive -Path "$package/*" -DestinationPath (Join-Path $dist "listenbox-desktop-windows-$Architecture.zip") -Force

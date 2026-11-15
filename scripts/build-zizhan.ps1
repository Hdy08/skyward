# Build the local stock Xiaomi MIX Fold 2 port without writing to a device.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Base,
    [string]$Ndk = $env:ANDROID_NDK_HOME,
    [string]$Lz4 = 'lz4',
    [string]$Out
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$config = Join-Path $repo 'devices/zizhan.toml'
$baseFile = (Resolve-Path -LiteralPath $Base).Path
$expected = [regex]::Match((Get-Content -LiteralPath $config -Raw), '(?m)^sha256\s*=\s*"([a-f0-9]{64})"\s*$').Groups[1].Value
if (-not $expected -or (Get-FileHash -LiteralPath $baseFile -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'The base does not match the stock recovery SHA-256 pinned in devices/zizhan.toml.'
}
if (-not $Ndk) {
    throw 'Pass -Ndk with the Android NDK directory, or set ANDROID_NDK_HOME.'
}
$llvm = Join-Path (Resolve-Path -LiteralPath $Ndk).Path 'toolchains/llvm/prebuilt/windows-x86_64/bin'
$clang = Join-Path $llvm 'aarch64-linux-android30-clang.cmd'
if (-not (Test-Path -LiteralPath $clang)) {
    throw "The NDK has no Android API 30 ARM64 compiler at $clang"
}
if (-not $Out) {
    $Out = Join-Path $repo 'target/zizhan/skyward-zizhan-experimental.img'
}
$outFile = [System.IO.Path]::GetFullPath($Out)
$saved = @{}
foreach ($name in @('PATH', 'CC_aarch64_linux_android', 'AR_aarch64_linux_android', 'CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER', 'RUSTFLAGS')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

function Invoke-Native([string]$Executable, [string[]]$Arguments) {
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable failed with exit code $LASTEXITCODE"
    }
}

Push-Location -LiteralPath $repo
try {
    $env:PATH = "$llvm;$(Join-Path $env:USERPROFILE '.cargo/bin');$env:PATH"
    $lz4Program = (Get-Command $Lz4 -CommandType Application -ErrorAction Stop).Source
    $env:PATH = "$(Split-Path -Parent $lz4Program);$env:PATH"
    $env:CC_aarch64_linux_android = $clang
    $env:AR_aarch64_linux_android = Join-Path $llvm 'llvm-ar.exe'
    $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $clang
    Invoke-Native 'cargo' @('build', '--locked', '--release', '-p', 'skyward-pack')
    Invoke-Native 'cargo' @('build', '--locked', '--release', '-p', 'skyward', '--target', 'aarch64-linux-android')
    $env:RUSTFLAGS = "$($saved['RUSTFLAGS']) -C target-feature=+crt-static".Trim()
    Invoke-Native 'cargo' @('build', '--locked', '--release', '-p', 'skyward-system-init', '--target', 'aarch64-linux-android', '--target-dir', 'target/system-init')
    [Environment]::SetEnvironmentVariable('RUSTFLAGS', $saved['RUSTFLAGS'], 'Process')
    New-Item -ItemType Directory -Path (Split-Path -Parent $outFile) -Force | Out-Null
    Invoke-Native (Join-Path $repo 'target/release/skyward-pack.exe') @(
        'recovery', '--base', $baseFile,
        '--binary', 'target/aarch64-linux-android/release/skyward',
        '--system-init', 'target/system-init/aarch64-linux-android/release/system-init',
        '--config', $config, '--font', 'resources/fonts/MiSansVF.ttf',
        '--mono', 'resources/fonts/KirkMono-Regular.ttf',
        '--mono-license', 'resources/fonts/KirkMono-OFL.txt', '--out', $outFile
    )
    $sum = (Get-FileHash -LiteralPath $outFile -Algorithm SHA256).Hash.ToLowerInvariant()
    "$sum  $([System.IO.Path]::GetFileName($outFile))" | Set-Content -LiteralPath "$outFile.sha256" -Encoding ascii
    Write-Output "Image: $outFile"
    Write-Output "SHA-256: $sum"
}
finally {
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
    }
    Pop-Location
}

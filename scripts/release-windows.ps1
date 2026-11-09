# Builds Sleeve.exe for 64-bit Windows, signs it when a certificate is set up, and packs it in a
# zip. The Windows counterpart of release-macos.sh.
#
#   pwsh scripts/release-windows.ps1
#
# Settings, all optional:
#   SIGN_CERT       the SHA-1 thumbprint of a code signing certificate in the current user's
#                   certificate store. Without one the exe isn't signed, and Windows SmartScreen
#                   warns about it the first time it's opened.
#   TIMESTAMP_URL   the timestamp server for signing (default: http://timestamp.digicert.com)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$Name = 'Sleeve'
$Target = 'x86_64-pc-windows-msvc'
$Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$Out = 'target/release-windows'
$Exe = "$Out/$Name.exe"
$Zip = "$Out/$Name-$Version-windows-x64.zip"
$TimestampUrl = if ($env:TIMESTAMP_URL) { $env:TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }

# Stops on a failed native command. Unlike a cmdlet, a program that fails doesn't stop the
# script by itself, even with ErrorActionPreference set.
function Invoke-Checked {
    param([string]$What)
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)." }
}

# The Windows SDK's tools, which the C++ build tools install. Takes the newest.
function Find-SdkTool {
    param([string]$Tool)
    $found = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\$Tool" -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Directory.Parent.Name } -ErrorAction SilentlyContinue |
        Select-Object -Last 1
    if (-not $found) { throw "$Tool not found. It comes with the Windows SDK." }
    $found.FullName
}

Write-Host "Building $Name $Version for 64-bit Windows..."
# The C runtime is linked in, so the exe runs on a PC without the Visual C++ Redistributable.
# With --target, RUSTFLAGS reach only what goes in the exe, not build scripts, and the build
# lands in its own folder rather than redoing the everyday one.
$env:RUSTFLAGS = '-C target-feature=+crt-static'
# A windowed app, not a console one: no black terminal window opens beside it. Passed only to
# the final link, the way `cargo rustc` does, so it doesn't change anything else. The icon
# and version info are build.rs's doing, so every build has them, not only this one.
cargo rustc --release --locked --target $Target --bin sleeve -- -C link-arg=/SUBSYSTEM:WINDOWS -C link-arg=/ENTRY:mainCRTStartup
Invoke-Checked 'cargo build'
Remove-Item Env:RUSTFLAGS

if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null
Copy-Item "target/$Target/release/sleeve.exe" $Exe

if ($env:SIGN_CERT) {
    $signtool = Find-SdkTool signtool.exe
    Write-Host "Signing with certificate $env:SIGN_CERT..."
    & $signtool sign /sha1 $env:SIGN_CERT /fd SHA256 /tr $TimestampUrl /td SHA256 $Exe
    Invoke-Checked 'signtool sign'
    & $signtool verify /pa $Exe
    Invoke-Checked 'signtool verify'
} else {
    Write-Warning 'No SIGN_CERT set, so the exe is not signed. SmartScreen will warn when it is first opened.'
}

Write-Host 'Making the zip...'
Compress-Archive -Path $Exe -DestinationPath $Zip -Force

Write-Host "Done: $Zip"

$ErrorActionPreference = "Stop"
cargo build --release
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\vaultpack.exe dist\Vaultpack-Portable.exe -Force
$nsis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
if (Test-Path $nsis) { & $nsis installer\vaultpack.nsi } else { Write-Host "NSIS not found, installer skipped" }

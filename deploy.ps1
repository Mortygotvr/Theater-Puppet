# Copies Theater Puppet into OBS's plugin folders (per machine and per user).
# Close OBS first. Usage: cargo build --release; .\deploy.ps1
param([string]$QtShim = "$PSScriptRoot\..\theater scene(rust)\Trust\target\release\trust_qt.dll")
$ErrorActionPreference = 'Stop'

$dll = "$PSScriptRoot\target\release\theater_puppet.dll"
if (!(Test-Path $dll)) { throw "$dll not found - run cargo build --release first." }
if (Get-Process obs64 -ErrorAction SilentlyContinue) { throw 'OBS is running - close it first.' }

foreach ($root in @("C:\ProgramData\obs-studio\plugins\theater-puppet", "$env:APPDATA\obs-studio\plugins\theater-puppet")) {
    New-Item -ItemType Directory -Path "$root\bin\64bit" -Force | Out-Null
    New-Item -ItemType Directory -Path "$root\data" -Force | Out-Null
    Copy-Item $dll "$root\bin\64bit\theater-puppet.dll" -Force
    # Theater Scene's Qt shim, for its item in the Theater menu (without it: OBS's Tools menu)
    if (Test-Path $QtShim) { Copy-Item $QtShim "$root\bin\64bit\trust_qt.dll" -Force }
    # The styles it ships with (yours go in plugin_config\theater-puppet\styles)
    New-Item -ItemType Directory -Path "$root\data\styles" -Force | Out-Null
    Copy-Item "$PSScriptRoot\styles\*.rhai" "$root\data\styles\" -Force
    # The filters it ships with (Theater Scene's are read from its own folders)
    New-Item -ItemType Directory -Path "$root\data\filters" -Force | Out-Null
    Copy-Item "$PSScriptRoot\filters\*.rhai" "$root\data\filters\" -Force
}
Write-Host "=== C:\ProgramData\obs-studio\plugins\theater-puppet ==="
Get-ChildItem -Path "C:\ProgramData\obs-studio\plugins\theater-puppet" -Recurse -File | Format-Table FullName, LastWriteTime, Length

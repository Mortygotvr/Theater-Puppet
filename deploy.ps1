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
    # MediaPipe's helper (run by a Python that has mediapipe and opencv)
    New-Item -ItemType Directory -Path "$root\data\mediapipe" -Force | Out-Null
    Copy-Item "$PSScriptRoot\mediapipe\*.py" "$root\data\mediapipe\" -Force
    # Its dock, a Theater Scene Rhai dock (Scene loads every plugin's data\theater-scripts)
    New-Item -ItemType Directory -Path "$root\data\theater-scripts" -Force | Out-Null
    Copy-Item "$PSScriptRoot\theater-scripts\*.rhai" "$root\data\theater-scripts\" -Force
    # The built-in controller the avatar holds (controller\make_gamepad.py makes it)
    New-Item -ItemType Directory -Path "$root\data\controller" -Force | Out-Null
    Copy-Item "$PSScriptRoot\controller\gamepad.glb" "$root\data\controller\" -Force
    # Its Theater Control Panel module (loaded from the Panel's Modules tab)
    New-Item -ItemType Directory -Path "$root\data\control-panel" -Force | Out-Null
    Copy-Item "$PSScriptRoot\control-panel\*.tcp" "$root\data\control-panel\" -Force
    # The filters it ships with (Theater Scene's are read from its own folders)
    New-Item -ItemType Directory -Path "$root\data\filters" -Force | Out-Null
    Copy-Item "$PSScriptRoot\filters\*.rhai" "$root\data\filters\" -Force
}
Write-Host "=== C:\ProgramData\obs-studio\plugins\theater-puppet ==="
Get-ChildItem -Path "C:\ProgramData\obs-studio\plugins\theater-puppet" -Recurse -File | Format-Table FullName, LastWriteTime, Length

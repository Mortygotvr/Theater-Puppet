# Builds the release zip, dist\theater-puppet-<version>-windows-x64.zip, for
# the Theater Installer (a release in Theater-Releases tagged
# theater-puppet-v<version>):
#   theater-puppet\bin\64bit   the plugin (theater-puppet.dll) and Theater Scene's Qt shim (trust_qt.dll)
#   theater-puppet\data        what deploy.ps1 installs: styles, filters, the
#                              MediaPipe helper, the docks, the controller, the
#                              Control Panel module (only the files git tracks)
# Build first: cargo build --release (and Theater Scene's trust_qt.dll).
# The version is Cargo.toml's unless given.
param(
    [string]$Version = "",
    [string]$QtShim = "$PSScriptRoot\..\theater scene(rust)\Trust\target\release\trust_qt.dll"
)
$ErrorActionPreference = 'Stop'

$root = $PSScriptRoot
if (!$Version) {
    $Version = (Select-String -LiteralPath "$root\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
}
$dll = "$root\target\release\theater_puppet.dll"
foreach ($f in @($dll, $QtShim)) {
    if (!(Test-Path -LiteralPath $f)) { throw "$f not found - build it first." }
}
$work = "$root\dist\stage"
$stage = "$work\theater-puppet"
if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory -Force "$stage\bin\64bit", "$stage\data" | Out-Null
Copy-Item -LiteralPath $dll "$stage\bin\64bit\theater-puppet.dll"
Copy-Item -LiteralPath $QtShim "$stage\bin\64bit\trust_qt.dll"

# Only what git tracks (and, of the controller, only the model)
$pairs = @(
    @("styles", "styles", "*.rhai"), @("filters", "filters", "*.rhai"), @("mediapipe", "mediapipe", "*.py"),
    @("theater-scripts", "theater-scripts", "*.rhai"), @("controller", "controller", "*.glb"), @("control-panel", "control-panel", "*.tcp")
)
foreach ($pair in $pairs) {
    foreach ($file in (git -C $root -c core.quotepath=off ls-files -- $pair[0])) {
        if ((Split-Path -Leaf $file) -notlike $pair[2]) { continue }
        $dest = Join-Path "$stage\data\$($pair[1])" $file.Substring($pair[0].Length + 1)
        New-Item -ItemType Directory -Force -Path (Split-Path -LiteralPath $dest) | Out-Null
        Copy-Item -LiteralPath (Join-Path $root $file) -Destination $dest
    }
}
Copy-Item -LiteralPath "$root\LICENSE" "$stage\LICENSE.txt"
# The version, which the Theater Installer reads to offer updates
Set-Content -LiteralPath "$stage\version.txt" -Value $Version -Encoding ascii

$zip = "$root\dist\theater-puppet-$Version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
# Entries named with / between folders, as zip tools expect (in Windows
# PowerShell, Compress-Archive and ZipFile.CreateFromDirectory put \)
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::Open($zip, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    $base = $work.Length + 1
    foreach ($file in Get-ChildItem -LiteralPath $stage -Recurse -File) {
        $name = $file.FullName.Substring($base).Replace('\', '/')
        [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $name, [System.IO.Compression.CompressionLevel]::Optimal)
    }
} finally { $archive.Dispose() }
Remove-Item $work -Recurse -Force
Write-Host "Made $zip"

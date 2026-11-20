# Builds the release app and packages it: target/package/LabSetup/ + LabSetup-<version>-portable.zip,
# plus an MSI when WiX 4+ (wix.exe) is on PATH or named by $env:LABSETUP_WIX.
# Env: CARGO_TARGET_DIR (shared target dir), LABSETUP_STEP2GLB_DIR (dir holding step2glb.exe + DLLs;
# default tools/step2glb/build/Release), LABSETUP_SKIP_BUILD=1 (reuse target/release/labsetup.exe).
$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..')
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$version = (Select-String -Path (Join-Path $repo 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value

if ($env:LABSETUP_SKIP_BUILD -ne '1') {
    Push-Location $repo
    try { cargo build --release --package lab-app --offline; if ($LASTEXITCODE) { throw 'cargo build failed' } }
    finally { Pop-Location }
}

$exe = Join-Path $target 'release/labsetup.exe'
$cli = Join-Path $target 'release/labsetup-cli.exe'
$sidecar = if ($env:LABSETUP_STEP2GLB_DIR) { $env:LABSETUP_STEP2GLB_DIR } else { Join-Path $repo 'tools/step2glb/build/Release' }
foreach ($p in @($exe, $cli, (Join-Path $sidecar 'step2glb.exe'), (Join-Path $repo 'third_party/dxc/dxcompiler.dll'))) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

# Output goes under the repo's target/ (not the shared target dir) so each checkout packages separately.
$pkgRoot = Join-Path $repo 'target/package'
$stage = Join-Path $pkgRoot 'LabSetup'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force "$stage/step2glb" | Out-Null
Copy-Item $exe $stage
Copy-Item $cli $stage
Copy-Item (Join-Path $repo 'third_party/dxc/dxcompiler.dll') $stage
Copy-Item (Join-Path $repo 'third_party/dxc/LICENSE.txt') (Join-Path $stage 'dxc-LICENSE.txt')
Copy-Item (Join-Path $repo 'library') (Join-Path $stage 'library') -Recurse
Get-ChildItem $sidecar -File | Where-Object { $_.Extension -in '.exe', '.dll', '.txt' } | Copy-Item -Destination "$stage/step2glb"

$zip = Join-Path $pkgRoot "LabSetup-$version-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "Portable zip: $zip"

$wix = if ($env:LABSETUP_WIX) { $env:LABSETUP_WIX } else { (Get-Command wix.exe -ErrorAction SilentlyContinue).Source }
if ($wix -and (Test-Path $wix)) {
    $msi = Join-Path $pkgRoot "LabSetup-$version.msi"
    & $wix build (Join-Path $PSScriptRoot 'labsetup.wxs') -arch x64 -d "Version=$version" -d "StageDir=$stage" -o $msi
    if ($LASTEXITCODE) { throw 'wix build failed' }
    Write-Host "MSI: $msi"
} else {
    Write-Host 'MSI skipped: WiX not found'
}

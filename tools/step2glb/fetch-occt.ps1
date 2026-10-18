# Fetches the official OpenCascade 7.9.3 prebuilt for MSVC x64 (vc14 = VS 2015-2022 ABI)
# from the OCCT GitHub releases and unpacks only what step2glb needs.
#
#   powershell -ExecutionPolicy Bypass -File tools\step2glb\fetch-occt.ps1 [-Dest D:\dev\occt-7.9.3] [-KeepZips]
#
# Downloads ~250 MB (two zips, SHA-256 checked) and leaves ~210 MB in -Dest:
#   <Dest>\opencascade-7.9.3-vc14-64\   headers, import libs, DLLs, CMake config
#   <Dest>\3rdparty-vc14-64\...\bin\     the 9 runtime DLLs the OCCT toolkits link
# Uses only curl.exe and tar.exe, both shipped with Windows 10/11.

param(
    [string]$Dest = "D:\dev\occt-7.9.3",
    [switch]$KeepZips
)
$ErrorActionPreference = "Stop"

$release = "https://github.com/Open-Cascade-SAS/OCCT/releases/download/V7_9_3"
$zips = @(
    @{ Name = "opencascade-7.9.3-vc14-64.zip"; Sha256 = "2a4f5e187992427674869e89477882d1ace4d67695d4b27173830d93f8cf11be" },
    @{ Name = "3rdparty-vc14-64.zip";          Sha256 = "f25be51af6160a2db74d19a4adc0c08728517822167b01459f8e775927a2a2f6" }
)

# TKernel links tbb12 + jemalloc; TKService (pulled in by TKXCAF/TKRWMesh) links freetype,
# FreeImage, openvr and ffmpeg. step2glb never calls the image/video/VR code, but the
# loader still needs the DLLs.
$thirdParty = @(
    "3rdparty-vc14-64/tbb-2021.13.0-x64/bin/tbb12.dll",
    "3rdparty-vc14-64/jemalloc-vc14-64/bin/jemalloc.dll",
    "3rdparty-vc14-64/freetype-2.13.3-x64/bin/freetype.dll",
    "3rdparty-vc14-64/freeimage-3.18.0-x64/bin/FreeImage.dll",
    "3rdparty-vc14-64/openvr-1.14.15-64/bin/win64/openvr_api.dll",
    "3rdparty-vc14-64/ffmpeg-3.3.4-64/bin/avcodec-57.dll",
    "3rdparty-vc14-64/ffmpeg-3.3.4-64/bin/avformat-57.dll",
    "3rdparty-vc14-64/ffmpeg-3.3.4-64/bin/avutil-55.dll",
    "3rdparty-vc14-64/ffmpeg-3.3.4-64/bin/swscale-4.dll"
)

$curl = Join-Path $env:SystemRoot "System32\curl.exe"
$tar  = Join-Path $env:SystemRoot "System32\tar.exe"
$dl   = Join-Path $Dest "_download"
New-Item -ItemType Directory -Force $dl | Out-Null

foreach ($z in $zips) {
    $path = Join-Path $dl $z.Name
    $ok = (Test-Path $path) -and ((Get-FileHash -Algorithm SHA256 $path).Hash -eq $z.Sha256)
    if (-not $ok) {
        Write-Host "Downloading $($z.Name) ..."
        & $curl -L --fail --retry 3 -o $path "$release/$($z.Name)"
        if ($LASTEXITCODE -ne 0) { throw "curl failed for $($z.Name)" }
        $hash = (Get-FileHash -Algorithm SHA256 $path).Hash
        if ($hash -ne $z.Sha256) { throw "SHA-256 mismatch for $($z.Name): got $hash" }
    }
}

Write-Host "Unpacking OpenCascade ..."
& $tar -xf (Join-Path $dl $zips[0].Name) -C $Dest
if ($LASTEXITCODE -ne 0) { throw "tar failed on $($zips[0].Name)" }

Write-Host "Unpacking the 3rd-party runtime DLLs ..."
& $tar -xf (Join-Path $dl $zips[1].Name) -C $Dest @thirdParty
if ($LASTEXITCODE -ne 0) { throw "tar failed on $($zips[1].Name)" }

if (-not $KeepZips) { Remove-Item -Recurse -Force $dl }
Write-Host "OpenCascade ready in $Dest"

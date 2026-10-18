# step2glb

Command-line sidecar that converts a vendor STEP file into a GLB for LabSetup's part library
(see `docs/DESIGN.md` §5, "STEP import"). It is C++17 on OpenCascade 7.9.3 and runs as its own
process, so a crash on a bad STEP file cannot take the app down.

1. `STEPCAFControl_Reader` reads the file and keeps part names and colours (XCAF).
   Whatever unit the file uses (Thorlabs files are in inches) is converted to millimetres.
2. `BRepMesh_IncrementalMesh` meshes it (default 0.05 mm linear / 0.3 rad angular deflection).
3. `RWGltf_CafWriter` writes a binary glTF, one primitive per part and colour.

**Output conventions are LabSetup's, not glTF's: millimetres, +Z up** (+Y forward, +X right),
the same as `lab-core`'s document space. Other glTF viewers will show the part 1000× too large
and lying on its back unless you pass `--standard-gltf`.

## Build

Needs Windows 10/11 x64, Visual Studio 2022 Build Tools (MSVC v143 + Windows SDK) and
CMake ≥ 3.21. The CMake bundled with the Build Tools works:
`C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe`.
Run the commands from the repository root.

**1. Get OpenCascade** (once). Downloads the official 7.9.3 prebuilt for MSVC x64 from the
[OCCT GitHub releases](https://github.com/Open-Cascade-SAS/OCCT/releases/tag/V7_9_3): two zips,
~250 MB, SHA-256 checked. It keeps ~210 MB in `D:\dev\occt-7.9.3` (OpenCascade plus the nine
third-party DLLs it links) and deletes the zips. Nothing is installed system-wide.

```powershell
powershell -ExecutionPolicy Bypass -File tools\step2glb\fetch-occt.ps1
```

Use `-Dest <folder>` for another location, then pass `-DOCCT_ROOT=<folder>` when configuring.

**2. Configure and build** (Release only: the prebuilt has no debug libraries).

```powershell
cmake -S tools/step2glb -B tools/step2glb/build -G "Visual Studio 17 2022" -A x64
cmake --build tools/step2glb/build --config Release
```

`tools/step2glb/build/Release/` is then self-contained: `step2glb.exe`, 34 DLLs (~62 MB: 25
OpenCascade toolkits, plus freetype, tbb, jemalloc, FreeImage, openvr and four ffmpeg DLLs) and
the OpenCascade licence texts. It also needs the MSVC runtime (`VCRUNTIME140.dll`,
`MSVCP140.dll`), which the app's installer has to ship anyway.

Why FreeImage, ffmpeg and openvr: `TKService` links them, and the XCAF and glTF toolkits link
`TKService`. step2glb never calls that code, but Windows still loads the DLLs. A from-source
OpenCascade build without those options would drop ~25 MB.

## Usage

```
step2glb <input.step> <output.glb> [options]

  -l, --linear-deflection <mm>    max distance from mesh to surface (default 0.05)
  -a, --angular-deflection <rad>  max angle between adjacent facets (default 0.3)
      --source-up <y|z>           up axis of the STEP model (default z); y rotates it to
                                  +Z up (SolidWorks exports, e.g. Thorlabs, are y)
      --standard-gltf             write glTF's +Y up, metres instead (for other viewers)
  -v, --verbose                   print OpenCascade's progress messages to stderr
```

STEP files do not record which way is up. Every Thorlabs file tried so far is modelled Y-up
(SolidWorks' default), so the importer should pass `--source-up y` for Thorlabs parts.

On success it prints one line of JSON to stdout and exits 0:

```json
{"input":"D:\\dev\\step-samples\\RS99_M-Step.step","output":"D:\\dev\\step-samples\\out\\RS99_M.glb",
 "units":"mm","up_axis":"+Z","source_length_unit":"INCH","source_up_axis":"+Y",
 "parts":1,"instances":1,"faces":2534,"unmeshed_faces":0,"triangles":83008,"triangles_unique":83008,
 "bbox_mm":{"min":[-26.889,-67.957,-6.716],"max":[25.911,82.144,188.092]},"bytes":2469156,
 "seconds":{"read":0.887,"transfer":3.369,"mesh":0.568,"write":0.155,"total":4.986}}
```

`triangles` counts every instance; `triangles_unique` counts each part once (what the GLB
stores). `bbox_mm` is the mesh's extent in the output frame. The GLB's `asset.extras` records
`labsetup_units`, `labsetup_up_axis`, `labsetup_source_up_axis`, the deflections and the tool
version, so the importer can check what it got.

On failure it prints `step2glb: error: <reason>` to stderr (OpenCascade's own parser message,
if any, comes on the line before) and exits with:

| Code | Meaning |
|---|---|
| 1 | bad command line |
| 2 | input missing, empty, not a STEP file (a ZIP or `.stpZ` is named as such), or unparseable |
| 3 | STEP file read, but it has no shapes or no surfaces |
| 4 | meshing produced no triangles |
| 5 | output folder missing or GLB not writable |
| 6 | OpenCascade exception or crash (reported, never a Windows error dialog) |

The GLB is written as `<output>.partial` and renamed when complete, so a failed or killed run
never leaves a half-written file. The app runs the tool with a timeout (60 s in the design).

## Loading the GLB in the viewport

`viewport-spike.diff` is the throwaway change that loaded these GLBs into the LabSetup
viewport for the M0 check (not merged; it is M4 work). In short: enable Bevy's `bevy_gltf` and
`reflect_auto_register` features, spawn `WorldAssetRoot(GltfAssetLabel::Scene(0))` with
`rotation_to_render(r) * Quat::from_rotation_x(-FRAC_PI_2)` and scale 0.001, and keep the
on-demand loop rendering until the asynchronous load has spawned. Details in
`docs/DESIGN.md` §13.

## Licences

OpenCascade is LGPL-2.1 with the OCCT exception; it is linked dynamically, and its licence
texts are copied next to the DLLs. The third-party DLLs carry their own licences (FreeType,
FreeImage, FFmpeg LGPL, oneTBB Apache-2.0, jemalloc BSD, OpenVR BSD); the installer (M5) must
include those texts too.

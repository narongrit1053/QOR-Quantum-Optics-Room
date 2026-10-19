# DirectX Shader Compiler (DXC)

`dxcompiler.dll` must sit next to `labsetup.exe`. wgpu's default DX12 compiler setting
(`Dx12Compiler::Auto`) loads it from there. Without it wgpu falls back to FXC, which compiles
Bevy's pipelines so slowly that the viewport stays empty for 10–15 s at startup in a debug
build and longer in a release build (measured in POLA-28 / F2).

- Version 1.8.2502.11 x64, the official DXC v1.8.2502 release (wgpu 30 needs at least this version).
- Copied from the Windows SDK 10.0.26100 redistributables (`Redist/D3D/x64`), which the SDK licence allows
  to be redistributed. `dxil.dll` is not needed.
- SHA-256 `b855ee494c60a5dd0638d4574bc4d728c87fac5d415b9be3122058b5adefda77`.
- Licence: University of Illinois/NCSA (LLVM), see `LICENSE.txt`. Ship that file with the DLL.

`crates/lab-app/build.rs` copies the DLL next to the built executable.

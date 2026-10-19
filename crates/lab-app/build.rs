//! Puts `dxcompiler.dll` next to the executable so wgpu compiles shaders with DXC instead of the
//! much slower FXC (see `third_party/dxc/README.md`).

use std::path::Path;

fn main() {
    let dll = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/dxc/dxcompiler.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // OUT_DIR is <target>/<profile>/build/lab-app-<hash>/out; the executable goes to <target>/<profile>.
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let profile_dir = Path::new(&out_dir).ancestors().nth(3).expect("OUT_DIR is inside the target directory");
    if let Err(error) = std::fs::copy(&dll, profile_dir.join("dxcompiler.dll")) {
        println!("cargo:warning=could not copy dxcompiler.dll next to the executable: {error}");
    }
}

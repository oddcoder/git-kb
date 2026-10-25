//! Compiles the browser code, `web/src/*.ts` and `plugins/*/*.ts`, into cargo's folder for what a build
//! makes, where the binary takes it from: see `web/build.mjs`, which this runs, and which needs Node. A package
//! of the crate, as crates.io has it, has them compiled already, in `web/dist/`, which this copies instead, so that
//! installing it needs no Node; a checkout has no `web/dist/`, as git ignores it.

use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=web");
    println!("cargo::rerun-if-changed=plugins");
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let dist = Path::new("web").join("dist");
    if dist.is_dir() {
        copy(&dist, Path::new(&out)).expect("copying the compiled browser code");
        return;
    }
    let status = Command::new("node")
        .arg(Path::new("web").join("build.mjs"))
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("node, to compile the browser code: {e}; is Node installed?"));
    assert!(
        status.success(),
        "compiling the browser code failed: {status}"
    );
}

/// A folder's files, and its folders', into another.
fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let into = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy(&entry.path(), &into)?;
        } else {
            fs::copy(entry.path(), into)?;
        }
    }
    Ok(())
}

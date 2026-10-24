//! Compiles the browser code, `web/src/*.ts` and `plugins/*/*.ts`, into cargo's folder for what a build
//! makes, where the binary takes it from: see `web/build.mjs`, which this runs, and which needs Node.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=web");
    println!("cargo::rerun-if-changed=plugins");
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
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

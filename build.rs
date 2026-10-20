//! Compiles the browser code, `web/src/*.ts`, into cargo's folder for what a build makes, where the binary takes
//! it from: see `web/build.mjs`, which this runs, and which needs Node.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=web");
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let status = Command::new("node")
        .arg(Path::new("web").join("build.mjs"))
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("node, to compile web/src/*.ts: {e}; is Node installed?"));
    assert!(status.success(), "compiling web/src/*.ts failed: {status}");
}

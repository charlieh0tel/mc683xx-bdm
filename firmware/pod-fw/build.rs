//! Puts the chip's memory map where cortex-m-rt's link.x finds it, as
//! memory.x, and links embassy-rp's boot2 script for the RP2040 and
//! defmt's with the `rtt` feature; puts the git commit in the version
//! the pod reports.

use std::env;
use std::fs;
use std::path::PathBuf;

include!("../../crates/git-version.rs");

fn main() {
    println!("cargo:rustc-env=GIT_VERSION={}", git_version());
    let rp2040 = env::var_os("CARGO_FEATURE_RP2040").is_some();
    let memory = if rp2040 { "memory-rp2040.x" } else { "memory-rp235x.x" };
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::copy(memory, out.join("memory.x")).expect("memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-rp2040.x");
    println!("cargo:rerun-if-changed=memory-rp235x.x");
    if rp2040 {
        println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
    }
    if env::var_os("CARGO_FEATURE_RTT").is_some() {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    }
}

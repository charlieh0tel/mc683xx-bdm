//! Puts the git commit in `bdm`'s version.

include!("../../firmware/git-version.rs");

fn main() {
    println!("cargo:rustc-env=GIT_VERSION={}", git_version());
}

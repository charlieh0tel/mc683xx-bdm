// Included by pod-fw's and pod-host's build scripts: the commit a crate
// is built from, for the version it reports ("0.1.0 1a2b3c4", "-dirty"
// with uncommitted changes, "unknown" outside a git checkout).

/// The short commit hash, marked dirty if tracked files differ; makes
/// cargo rebuild when HEAD or the index moves.
fn git_version() -> String {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    if let Some(dir) = run(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
        if let Some(head) = run(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{head}");
        }
    }
    let Some(hash) = run(&["rev-parse", "--short=7", "HEAD"]) else {
        println!("cargo:warning=no git commit to put in the version");
        return "unknown".to_owned();
    };
    let dirty = run(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|status| !status.is_empty());
    if dirty { format!("{hash}-dirty") } else { hash }
}

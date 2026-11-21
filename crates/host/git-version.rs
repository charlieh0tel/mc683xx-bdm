// Included by pod-fw's and mc683xx-bdm's build scripts: the commit a crate
// is built from, for the version it reports ("0.1.0 1a2b3c4", "-dirty"
// with uncommitted changes, "unknown" outside a git checkout).  A
// crate built from its published package takes the commit cargo
// recorded in it.

/// The commit a published package was made from: `sha1` in the
/// `.cargo_vcs_info.json` that `cargo package` writes into it, short,
/// and marked dirty if it says so (`--allow-dirty`).
fn packaged_version() -> Option<String> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let info = std::fs::read_to_string(std::path::Path::new(&manifest_dir).join(".cargo_vcs_info.json")).ok()?;
    let (_, after_key) = info.split_once("\"sha1\"")?;
    let hash = after_key.split('"').nth(1)?.get(..7)?;
    let dirty = info.contains("\"dirty\": true");
    Some(if dirty { format!("{hash}-dirty") } else { hash.to_owned() })
}

/// The short commit hash, marked dirty if tracked files differ; a
/// published package's commit ([`packaged_version`]) first.  Makes
/// cargo rerun the build script when HEAD, the branch's ref (loose or
/// packed), the index or any tracked file changes, in a linked worktree
/// too: `--git-path` names each where git keeps it.
fn git_version() -> String {
    if let Some(hash) = packaged_version() {
        return hash;
    }
    // --no-optional-locks: status must not rewrite the index it watches,
    // or the next build reruns this script again.
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("--no-optional-locks")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_owned())
    };
    // A path cargo cannot watch (a newline in it) or one that does not
    // exist (cargo would rerun on every build) is left out.
    let watch = |path: &str| {
        if !path.contains('\n') && std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    };
    let git_path = |path: &str| run(&["rev-parse", "--path-format=absolute", "--git-path", path]);
    if let Some(top) = run(&["rev-parse", "--show-toplevel"]) {
        let head = run(&["symbolic-ref", "-q", "HEAD"]);
        let refs = ["HEAD", "index", "packed-refs"].into_iter().map(str::to_owned).chain(head);
        refs.filter_map(|path| git_path(&path)).for_each(|path| watch(&path));
        if let Some(files) = run(&["-C", &top, "ls-files", "-z", "--recurse-submodules"]) {
            files.split('\0').filter(|file| !file.is_empty()).for_each(|file| watch(&format!("{top}/{file}")));
        }
    }
    let Some(hash) = run(&["rev-parse", "--short=7", "HEAD"]) else {
        println!("cargo:warning=no git commit to put in the version");
        return "unknown".to_owned();
    };
    let dirty = run(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|status| !status.trim().is_empty());
    if dirty { format!("{hash}-dirty") } else { hash }
}

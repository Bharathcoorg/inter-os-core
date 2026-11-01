//! Declares the kernel's real build inputs.
//!
//! `src/task/images.rs` embeds the first-party Ring-3 user images with
//! `include_bytes!`. Cargo does not track files reached through `include_bytes!`,
//! so without this the kernel would keep whatever bytes were on disk when it was
//! last compiled and silently ship a stale launcher, compositor or service after
//! its source changed. Emitting `rerun-if-changed` for every embedded image makes
//! that dependency explicit.
//!
//! WHY the list must be exactly the embedded set, and not merely a superset:
//! `rerun-if-changed` for a path that does not exist makes cargo dirty the crate
//! on every build -- it cannot fingerprint what it cannot stat. An entry for an
//! image that was removed therefore costs a full kernel recompile on every
//! no-op rebuild, while a missing entry lets an edited image ship the bytes the
//! previous build embedded. Both are silent, and both are this list's only job.

use std::path::Path;

/// Every user image embedded by `src/task/images.rs`, keyed by output file name.
const EMBEDDED_USER_IMAGES: &[&str] = &[
    "init",
    "settings",
    "displayd",
    "inputd",
    "storaged",
    "netd",
    "powerd",
    "modemd",
    "audiod",
    "camerad",
    "sensord",
    "compositor",
    "launcher",
    "dialer",
    "vault",
    "camera",
    "calculator",
    "browser",
    "messages",
    "calendar",
    "music",
    "file_manager",
    "gallery",
    "notes",
    "agent_manager",
    "video_player",
    "webview_demo",
    "aid",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => Path::new(&dir).to_path_buf(),
        Err(_) => return,
    };
    let user_target = manifest_dir.join("..").join("target").join("x86_64-unknown-none").join("debug");

    for image in EMBEDDED_USER_IMAGES {
        let path = user_target.join(image);
        // Track the path whether or not it exists yet, so creating a missing
        // image later also triggers a rebuild.
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let missing: Vec<&str> = EMBEDDED_USER_IMAGES
        .iter()
        .filter(|image| !user_target.join(image).exists())
        .copied()
        .collect();
    if !missing.is_empty() {
        // Only a warning: the `include_bytes!` below still fails loudly at compile
        // time, and this message names exactly which build step was skipped.
        println!(
            "cargo:warning=user images missing from {}: {} (run tools/build_userspace.ps1)",
            user_target.display(),
            missing.join(", ")
        );
    }
}

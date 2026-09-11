use chrono::Utc;
use regex::Regex;
use std::{env, path::PathBuf, process::Command};

fn repository_dir() -> PathBuf {
    PathBuf::from("../..")
        .canonicalize()
        .expect("KDF repository root must exist")
}

fn git_output(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(repository_dir())
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run git command: {error}"))?;
    if !output.status.success() {
        return Err("Git command failed".to_owned());
    }
    String::from_utf8(output.stdout)
        .map(|output| output.trim().to_owned())
        .map_err(|error| format!("Invalid UTF-8 sequence: {error}"))
}

fn crate_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn version_tag() -> Result<String, String> {
    if let Ok(tag) = env::var("KDF_BUILD_TAG") {
        return Ok(tag);
    }

    let commit_hash = git_output(&["log", "--pretty=format:%h", "-n1"])
        .map_err(|error| format!("{error}\nSet `KDF_BUILD_TAG` manually instead."))?;

    if !Regex::new(r"^\w+$")
        .expect("Failed to compile regex")
        .is_match(&commit_hash)
    {
        return Err(format!("Invalid tag: {commit_hash}"));
    }

    Ok(commit_hash)
}

fn version() -> Result<String, String> {
    version_tag().map(|tag| format!("{}_{}", crate_version(), tag))
}

fn build_datetime() -> String {
    Utc::now().to_rfc3339()
}

fn set_build_variables() -> Result<(), String> {
    println!("cargo:rustc-env=KDF_VERSION={}", version()?);
    println!("cargo:rustc-env=KDF_DATETIME={}", build_datetime());
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    println!("cargo:rerun-if-env-changed=KDF_BUILD_TAG");
    let repository_dir = repository_dir();
    if let Ok(head) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={}", repository_dir.join(head).display());
    }
    if let Ok(branch_ref) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
        if let Ok(branch_path) = git_output(&["rev-parse", "--git-path", &branch_ref]) {
            println!("cargo:rerun-if-changed={}", repository_dir.join(branch_path).display());
        }
    }
    if let Ok(packed_refs) = git_output(&["rev-parse", "--git-path", "packed-refs"]) {
        println!("cargo:rerun-if-changed={}", repository_dir.join(packed_refs).display());
    }

    set_build_variables().expect("Failed to set build variables");
}

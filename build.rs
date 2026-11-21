use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=android-agent/aether-agent.dex");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rc_file = manifest_dir.join("assets").join("icon.rc");
    let ico_file = manifest_dir.join("assets").join("icon.ico");

    if !rc_file.is_file() || !ico_file.is_file() {
        return;
    }

    let Some(rc_exe) = find_windows_rc_exe() else {
        println!("cargo:warning=rc.exe not found in Windows SDK; skipping .exe icon embedding");
        return;
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let res_out = out_dir.join("icon.res");

    let status = Command::new(rc_exe)
        .args([
            "/nologo",
            "/fo",
            res_out.to_str().unwrap(),
            rc_file.to_str().unwrap(),
        ])
        .current_dir(manifest_dir.join("assets"))
        .status();

    if matches!(status, Ok(s) if s.success()) && res_out.is_file() {
        println!("cargo:rustc-link-arg-bins={}", res_out.display());
    } else {
        println!("cargo:warning=rc.exe failed to compile assets/icon.rc");
    }
}

fn find_windows_rc_exe() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }

    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let Ok(entries) = fs::read_dir(kits_bin) else {
        return None;
    };

    let mut candidates: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("x64").join("rc.exe"))
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    candidates.pop()
}

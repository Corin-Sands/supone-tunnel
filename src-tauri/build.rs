// 作者: Corin-Sands
//! 任意打包/编译前确保 externalBin 内核在场：缺则跑 scripts/fetch-mihomo.mjs。
//! 与 beforeBuildCommand 同源，避免只走 cargo/android 时漏内核。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    ensure_mihomo_sidecar();
    tauri_build::build();
}

fn ensure_mihomo_sidecar() {
    let triple = std::env::var("TAURI_ENV_TARGET_TRIPLE")
        .or_else(|_| std::env::var("TARGET"))
        .unwrap_or_else(|_| default_host_triple());
    let ext = if triple.contains("windows") { ".exe" } else { "" };
    let bin = PathBuf::from(format!("binaries/mihomo-{triple}{ext}"));
    if bin.is_file() {
        if let Ok(meta) = std::fs::metadata(&bin) {
            if meta.len() > 1_000_000 {
                println!("cargo:rerun-if-changed={}", bin.display());
                return;
            }
        }
    }
    let script = PathBuf::from("../scripts/fetch-mihomo.mjs");
    if !script.is_file() {
        println!("cargo:warning=缺少 {}，externalBin 可能失败", script.display());
        return;
    }
    let status = Command::new("node")
        .arg(&script)
        .arg(&triple)
        .env("REGION", std::env::var("REGION").unwrap_or_else(|_| "else".into()))
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rerun-if-changed={}", bin.display());
        }
        Ok(s) => {
            println!(
                "cargo:warning=fetch-mihomo 退出码 {:?}，请手动: REGION=$REGION node scripts/fetch-mihomo.mjs {triple}",
                s.code()
            );
        }
        Err(err) => {
            println!("cargo:warning=无法执行 node fetch-mihomo: {err}");
        }
    }
}

fn default_host_triple() -> String {
    Command::new("rustc")
        .args(["--print", "host-tuple"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

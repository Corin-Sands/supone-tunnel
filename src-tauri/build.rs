// Author: Corin-Sands
//! 任意打包/编译前确保 externalBin 内核、country.mmdb、（Windows）wintun.dll 在场。
//! 与 beforeBuildCommand 同源，避免只走 cargo/android 时漏资源。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    ensure_mihomo_sidecar();
    ensure_country_mmdb();
    ensure_wintun();
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

fn ensure_country_mmdb() {
    let mmdb = PathBuf::from("resources/country.mmdb");
    if mmdb.is_file() {
        if let Ok(meta) = std::fs::metadata(&mmdb) {
            if meta.len() > 1_000_000 {
                println!("cargo:rerun-if-changed={}", mmdb.display());
                return;
            }
        }
    }
    let script = PathBuf::from("../scripts/fetch-geodata.mjs");
    if !script.is_file() {
        println!("cargo:warning=缺少 {}，bundle.resources 可能失败", script.display());
        return;
    }
    let status = Command::new("node")
        .arg(&script)
        .env("REGION", std::env::var("REGION").unwrap_or_else(|_| "else".into()))
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rerun-if-changed={}", mmdb.display());
        }
        Ok(s) => {
            println!(
                "cargo:warning=fetch-geodata 退出码 {:?}，请手动: REGION=$REGION node scripts/fetch-geodata.mjs",
                s.code()
            );
        }
        Err(err) => {
            println!("cargo:warning=无法执行 node fetch-geodata: {err}");
        }
    }
}

fn ensure_wintun() {
    let triple = std::env::var("TAURI_ENV_TARGET_TRIPLE")
        .or_else(|_| std::env::var("TARGET"))
        .unwrap_or_else(|_| default_host_triple());
    if !triple.contains("windows") {
        return;
    }
    let dll = PathBuf::from("resources/wintun.dll");
    if dll.is_file() {
        if let Ok(meta) = std::fs::metadata(&dll) {
            if meta.len() > 100_000 {
                println!("cargo:rerun-if-changed={}", dll.display());
                return;
            }
        }
    }
    let script = PathBuf::from("../scripts/fetch-wintun.mjs");
    if !script.is_file() {
        println!("cargo:warning=缺少 {}，Windows TUN 可能缺 wintun.dll", script.display());
        return;
    }
    let status = Command::new("node")
        .arg(&script)
        .arg(&triple)
        .env("REGION", std::env::var("REGION").unwrap_or_else(|_| "else".into()))
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rerun-if-changed={}", dll.display());
        }
        Ok(s) => {
            println!(
                "cargo:warning=fetch-wintun 退出码 {:?}，请手动: REGION=$REGION node scripts/fetch-wintun.mjs {triple}",
                s.code()
            );
        }
        Err(err) => {
            println!("cargo:warning=无法执行 node fetch-wintun: {err}");
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

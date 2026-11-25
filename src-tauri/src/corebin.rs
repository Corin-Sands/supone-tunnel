// Author: Corin-Sands
//! 只认 MetaCubeX 官方 mihomo。Clash Verge 自带内核一律拒绝。
//! 安装包经 Tauri externalBin / resources 携带 sidecar 与 country.mmdb；
//! 运行时落到 data/bin 与 profiles 根目录（可 setuid / 可写 / 离线 GEOIP）。

use std::fs::{self, File};
use std::io::{copy, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::Deserialize;

use crate::settings::reject_verge;

const RELEASE_API: &str = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
/// 小于此字节视为损坏或占位，需从安装包重拷。Author: Corin-Sands
const MIN_MMDB_BYTES: u64 = 1_000_000;

pub fn bundled_path(root: &Path) -> PathBuf {
    let name = if cfg!(windows) { "mihomo.exe" } else { "mihomo" };
    root.join("bin").join(name)
}

/// 安装包内 sidecar：与主程序同目录（Windows `mihomo.exe`，其它 `mihomo`）。
/// Author: Corin-Sands
pub fn find_packaged() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) { "mihomo.exe" } else { "mihomo" };
    let path = dir.join(name);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// mihomo `-d` 家目录下的 country.mmdb；observe 同步一份 geoip.metadb。
/// Author: Corin-Sands
pub fn country_mmdb_path(root: &Path) -> PathBuf {
    root.join("country.mmdb")
}

/// 安装包内 country.mmdb：Windows/Linux 与 exe 同级或 resources/；macOS 在 Contents/Resources。
/// Author: Corin-Sands
pub fn find_packaged_mmdb() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![
        dir.join("country.mmdb"),
        dir.join("resources").join("country.mmdb"),
    ];
    // macOS .app: Contents/MacOS → Contents/Resources
    if let Some(contents) = dir.parent() {
        let resources = contents.join("Resources");
        candidates.push(resources.join("country.mmdb"));
        candidates.push(resources.join("resources").join("country.mmdb"));
    }
    // cargo run / 开发：src-tauri/resources（相对当前工作目录或可执行上级）
    candidates.push(PathBuf::from("src-tauri/resources/country.mmdb"));
    candidates.push(PathBuf::from("resources/country.mmdb"));
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(manifest_dir).join("resources/country.mmdb"));
    }
    candidates.into_iter().find(|p| {
        p.is_file()
            && fs::metadata(p)
                .map(|m| m.len() >= MIN_MMDB_BYTES)
                .unwrap_or(false)
    })
}

/// 保证 profiles 根有 country.mmdb（及 geoip.metadb）：已有足够大则不动；否则从安装包拷入。
/// 避免启动时 mihomo 去拉 GitHub（无代理环境会卡住）。Author: Corin-Sands
pub fn ensure_country_mmdb(root: &Path) -> Result<(), String> {
    let dest = country_mmdb_path(root);
    if dest.is_file() {
        if let Ok(meta) = fs::metadata(&dest) {
            if meta.len() >= MIN_MMDB_BYTES {
                ensure_observe_alias(root, &dest)?;
                return Ok(());
            }
        }
    }
    let src = find_packaged_mmdb().ok_or_else(|| {
        "未找到内置 country.mmdb。请重新安装应用，或检查安装包 resources。".to_string()
    })?;
    fs::create_dir_all(root).map_err(|err| err.to_string())?;
    fs::copy(&src, &dest).map_err(|err| format!("安装内置 GeoIP 失败: {err}"))?;
    ensure_observe_alias(root, &dest)?;
    Ok(())
}

fn ensure_observe_alias(root: &Path, mmdb: &Path) -> Result<(), String> {
    let alias = root.join("geoip.metadb");
    if alias.is_file() {
        if let Ok(meta) = fs::metadata(&alias) {
            if meta.len() >= MIN_MMDB_BYTES {
                return Ok(());
            }
        }
    }
    fs::copy(mmdb, &alias).map_err(|err| format!("安装 geoip.metadb 失败: {err}"))?;
    Ok(())
}

/// Windows TUN：wintun.dll 必须与 mihomo.exe 同目录。Author: Corin-Sands
const MIN_WINTUN_BYTES: u64 = 100_000;

pub fn wintun_path(root: &Path) -> PathBuf {
    root.join("bin").join("wintun.dll")
}

/// 安装包内 wintun.dll（仅 Windows 包经 tauri.windows.conf resources 打入）。
/// Author: Corin-Sands
pub fn find_packaged_wintun() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![
        dir.join("wintun.dll"),
        dir.join("resources").join("wintun.dll"),
    ];
    if let Some(contents) = dir.parent() {
        let resources = contents.join("Resources");
        candidates.push(resources.join("wintun.dll"));
        candidates.push(resources.join("resources").join("wintun.dll"));
    }
    candidates.push(PathBuf::from("src-tauri/resources/wintun.dll"));
    candidates.push(PathBuf::from("resources/wintun.dll"));
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(manifest_dir).join("resources/wintun.dll"));
    }
    candidates.into_iter().find(|p| {
        p.is_file()
            && fs::metadata(p)
                .map(|m| m.len() >= MIN_WINTUN_BYTES)
                .unwrap_or(false)
    })
}

/// 保证 data/bin/wintun.dll 与 mihomo 同目录。Author: Corin-Sands
pub fn ensure_wintun(root: &Path) -> Result<PathBuf, String> {
    let dest = wintun_path(root);
    if dest.is_file() {
        if let Ok(meta) = fs::metadata(&dest) {
            if meta.len() >= MIN_WINTUN_BYTES {
                return Ok(dest);
            }
        }
    }
    let src = find_packaged_wintun().ok_or_else(|| {
        "未找到内置 wintun.dll。请重新安装 Windows 版应用。".to_string()
    })?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::copy(&src, &dest).map_err(|err| format!("安装 wintun.dll 失败: {err}"))?;
    Ok(dest)
}

/// 保证 data/bin 有可写内核：已有则用；否则从安装包 sidecar 拷入。
/// Author: Corin-Sands
pub fn ensure_staged(root: &Path) -> Result<PathBuf, String> {
    let dest = bundled_path(root);
    if dest.is_file() {
        reject_verge(&dest)?;
        return Ok(dest);
    }
    let src = find_packaged().ok_or_else(|| {
        "未找到官方 mihomo。请重新安装应用，或在设置里下载官方内核。".to_string()
    })?;
    reject_verge(&src)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::copy(&src, &dest).map_err(|err| format!("安装内置内核失败: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
            .map_err(|err| err.to_string())?;
    }
    reject_verge(&dest)?;
    Ok(dest)
}

pub fn resolve(root: &Path, configured: &str) -> Result<PathBuf, String> {
    if !configured.trim().is_empty() {
        let path = PathBuf::from(configured.trim());
        reject_verge(&path)?;
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("内核路径不存在: {}", path.display()));
    }
    ensure_staged(root).or_else(|stage_err| {
        let Ok(explicit) = std::env::var("MIHOMO_BIN") else {
            return Err(stage_err);
        };
        let path = PathBuf::from(explicit);
        reject_verge(&path)?;
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!("MIHOMO_BIN 不存在: {}", path.display()))
        }
    })
}

pub async fn download_official(root: &Path) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent("supone-tunnel")
        .build()
        .map_err(|err| err.to_string())?;
    let api = mirror_url(RELEASE_API);
    let release: Release = client
        .get(&api)
        .send()
        .await
        .map_err(|err| format!("读取 mihomo 发行版失败: {err}"))?
        .error_for_status()
        .map_err(|err| format!("读取 mihomo 发行版失败: {err}"))?
        .json()
        .await
        .map_err(|err| format!("解析 mihomo 发行版失败: {err}"))?;
    let want = asset_prefix();
    let asset = release
        .assets
        .iter()
        .find(|item| {
            item.name.starts_with(want)
                && item.name.ends_with(".gz")
                && !item.name.contains("compatible")
                && !item.name.contains("-go1")
                && !item.name.contains("-v1-")
                && !item.name.contains("-v2-")
                && !item.name.contains("-v3-")
        })
        .or_else(|| {
            release.assets.iter().find(|item| {
                item.name.starts_with(want) && item.name.ends_with(".gz") && !item.name.contains("compatible")
            })
        })
        .ok_or_else(|| format!("发行版 {} 没有匹配的官方包", release.tag_name))?;
    let url = mirror_url(&asset.browser_download_url);
    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("下载 mihomo 失败: {err}"))?
        .error_for_status()
        .map_err(|err| format!("下载 mihomo 失败: {err}"))?
        .bytes()
        .await
        .map_err(|err| format!("下载 mihomo 失败: {err}"))?;
    let target = bundled_path(root);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let tmp = target.with_extension("tmp");
    {
        let mut decoder = GzDecoder::new(bytes.as_ref());
        let mut file = File::create(&tmp).map_err(|err| err.to_string())?;
        copy(&mut decoder, &mut file).map_err(|err| format!("解压 mihomo 失败: {err}"))?;
        file.flush().map_err(|err| err.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|err| err.to_string())?;
    }
    fs::rename(&tmp, &target).map_err(|err| err.to_string())?;
    reject_verge(&target)?;
    Ok(target)
}

fn asset_prefix() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mihomo-darwin-arm64-"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "mihomo-darwin-amd64-"
    } else if cfg!(all(target_os = "android", target_arch = "x86_64")) {
        "mihomo-android-amd64-"
    } else if cfg!(all(target_os = "android", target_arch = "aarch64")) {
        "mihomo-android-arm64-v8-"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "mihomo-linux-arm64-"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "mihomo-linux-amd64-"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "mihomo-windows-amd64-"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "mihomo-windows-arm64-"
    } else {
        "mihomo-unsupported-"
    }
}

fn mirror_url(url: &str) -> String {
    if std::env::var("REGION").ok().as_deref() == Some("china") {
        format!("https://ghfast.top/{url}")
    } else {
        url.to_string()
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_verge_paths() {
        assert!(reject_verge(Path::new("/Applications/Clash Verge.app/Contents/MacOS/verge-mihomo")).is_err());
        assert!(reject_verge(Path::new("/data/bin/mihomo")).is_ok());
    }

    #[test]
    fn bundled_path_platform_name() {
        let p = bundled_path(Path::new("/tmp/data"));
        if cfg!(windows) {
            assert!(p.ends_with("bin\\mihomo.exe") || p.ends_with("bin/mihomo.exe"));
        } else {
            assert!(p.ends_with("bin/mihomo"));
        }
    }

    #[test]
    fn country_mmdb_path_at_profiles_root() {
        let p = country_mmdb_path(Path::new("/tmp/data"));
        assert!(p.ends_with("country.mmdb"));
    }
}

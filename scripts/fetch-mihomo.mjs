#!/usr/bin/env node
// 拉取 MetaCubeX 官方 mihomo 到 src-tauri/binaries/mihomo-$TRIPLE，供 Tauri externalBin 打包。
// Windows 官方包是 .zip；其它平台多为 .gz。CI 用 GITHUB_TOKEN 防 API 403。Author: Corin-Sands
import { execFileSync, execSync } from "node:child_process";
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  renameSync,
  statSync,
  readdirSync,
  copyFileSync,
  rmSync,
} from "node:fs";
import { unlink } from "node:fs/promises";
import path from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { createGunzip } from "node:zlib";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(root, "src-tauri", "binaries");
const region = process.env.REGION || "else";
const target =
  process.argv[2] ||
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.CARGO_BUILD_TARGET ||
  hostTriple();

const map = {
  "aarch64-apple-darwin": { prefix: "mihomo-darwin-arm64-", ext: "" },
  "x86_64-apple-darwin": { prefix: "mihomo-darwin-amd64-", ext: "" },
  "x86_64-pc-windows-msvc": { prefix: "mihomo-windows-amd64-", ext: ".exe" },
  "aarch64-pc-windows-msvc": { prefix: "mihomo-windows-arm64-", ext: ".exe" },
  "x86_64-unknown-linux-gnu": { prefix: "mihomo-linux-amd64-", ext: "" },
  "aarch64-unknown-linux-gnu": { prefix: "mihomo-linux-arm64-", ext: "" },
  "x86_64-linux-android": { prefix: "mihomo-android-amd64-", ext: "" },
  "aarch64-linux-android": { prefix: "mihomo-android-arm64-v8-", ext: "" },
  "armv7-linux-androideabi": { prefix: "mihomo-android-armv7-", ext: "" },
  "i686-linux-android": { prefix: "mihomo-android-386-", ext: "" },
};

const spec = map[target];
if (!spec) {
  console.error(`不支持的 target: ${target}`);
  process.exit(1);
}

mkdirSync(outDir, { recursive: true });
const dest = path.join(outDir, `mihomo-${target}${spec.ext}`);
if (existsSync(dest) && statSync(dest).size > 1_000_000) {
  console.log(`已有内核 ${dest} (${statSync(dest).size} bytes)`);
  process.exit(0);
}

const release = await loadRelease();
const asset = pickAsset(release.assets || [], spec.prefix);
if (!asset) {
  console.error(`发行版 ${release.tag_name} 无匹配包 prefix=${spec.prefix}`);
  process.exit(1);
}

const url = mirror(asset.browser_download_url);
console.log(`下载 ${asset.name} → ${dest}`);
// Expand-Archive 只认 .zip 后缀；勿用 .tmp。Author: Corin-Sands
const archive = asset.name.endsWith(".zip") ? `${dest}.dl.zip` : `${dest}.dl.gz`;
const tmpBin = `${dest}.bin.tmp`;
await download(url, archive);
await extractBinary(archive, asset.name, tmpBin, spec.ext);
await unlink(archive).catch(() => {});
renameSync(tmpBin, dest);
if (!spec.ext) {
  const { chmodSync } = await import("node:fs");
  chmodSync(dest, 0o755);
}
console.log(`就绪 ${dest} (${statSync(dest).size} bytes)`);

function hostTriple() {
  try {
    return execSync("rustc --print host-tuple", { encoding: "utf8" }).trim();
  } catch {
    const out = execSync("rustc -Vv", { encoding: "utf8" });
    const line = out.split("\n").find((l) => l.startsWith("host:"));
    if (!line) throw new Error("无法解析 rustc host");
    return line.split(/\s+/)[1];
  }
}

function mirror(url) {
  if (region !== "china") return url;
  if (url.includes("api.github.com")) return url;
  return `https://ghfast.top/${url}`;
}

function githubHeaders() {
  const headers = {
    "User-Agent": "supone-tunnel",
    Accept: "application/vnd.github+json",
  };
  const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

function pickAsset(assets, prefix) {
  const ok = (name, archive) =>
    name.startsWith(prefix) &&
    name.endsWith(archive) &&
    !name.includes("compatible") &&
    !name.includes("-go1") &&
    !name.includes("-v1-") &&
    !name.includes("-v2-") &&
    !name.includes("-v3-");
  return (
    assets.find((a) => ok(a.name, ".gz")) ||
    assets.find((a) => ok(a.name, ".zip")) ||
    assets.find(
      (a) =>
        a.name.startsWith(prefix) &&
        (a.name.endsWith(".gz") || a.name.endsWith(".zip")) &&
        !a.name.includes("compatible"),
    )
  );
}

async function loadRelease() {
  const api = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
  try {
    return await fetchJson(api);
  } catch (err) {
    const tag = process.env.MIHOMO_TAG || "v1.19.34";
    console.warn(`API 失败 (${err})，回退固定 tag ${tag}`);
    return {
      tag_name: tag,
      assets: [
        {
          name: `${spec.prefix}${tag}.gz`,
          browser_download_url: `https://github.com/MetaCubeX/mihomo/releases/download/${tag}/${spec.prefix}${tag}.gz`,
        },
        {
          name: `${spec.prefix}${tag}.zip`,
          browser_download_url: `https://github.com/MetaCubeX/mihomo/releases/download/${tag}/${spec.prefix}${tag}.zip`,
        },
      ],
    };
  }
}

async function fetchJson(url) {
  const res = await fetch(url, { headers: githubHeaders() });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`);
  return res.json();
}

async function download(url, file) {
  const urls = [url];
  if (region !== "china" && url.includes("github.com") && !url.includes("ghfast.top")) {
    urls.push(`https://ghfast.top/${url}`);
  }
  let lastErr;
  for (const u of urls) {
    try {
      const res = await fetch(u, {
        headers: { "User-Agent": "supone-tunnel" },
        signal: AbortSignal.timeout(180_000),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status} ${u}`);
      await pipeline(Readable.fromWeb(res.body), createWriteStream(file));
      return;
    } catch (err) {
      lastErr = err;
      console.warn(`下载失败，尝试下一个源: ${err}`);
    }
  }
  throw lastErr || new Error("下载失败");
}

async function extractBinary(archive, assetName, outFile, binExt) {
  if (assetName.endsWith(".gz")) {
    const { createReadStream } = await import("node:fs");
    await pipeline(createReadStream(archive), createGunzip(), createWriteStream(outFile));
    return;
  }
  if (assetName.endsWith(".zip")) {
    const extractDir = path.join(tmpdir(), `supone-mihomo-${process.pid}`);
    rmSync(extractDir, { recursive: true, force: true });
    mkdirSync(extractDir, { recursive: true });
    // Windows：GNU tar 会把 `D:` 当成远程主机；用 Expand-Archive。Author: Corin-Sands
    if (process.platform === "win32") {
      const ps = `Expand-Archive -LiteralPath '${archive.replace(/'/g, "''")}' -DestinationPath '${extractDir.replace(/'/g, "''")}' -Force`;
      execFileSync("powershell.exe", ["-NoProfile", "-Command", ps], {
        stdio: "inherit",
      });
    } else {
      execFileSync("tar", ["-xf", archive, "-C", extractDir], { stdio: "inherit" });
    }
    // 官方 zip 内常为 mihomo-windows-amd64.exe，不是 mihomo.exe。Author: Corin-Sands
    const found = findMihomoBinary(extractDir, binExt);
    if (!found) {
      throw new Error(`zip 内未找到 mihomo 可执行文件`);
    }
    copyFileSync(found, outFile);
    rmSync(extractDir, { recursive: true, force: true });
    return;
  }
  throw new Error(`不支持的包格式: ${assetName}`);
}

function findMihomoBinary(dir, binExt) {
  const files = [];
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      const full = path.join(d, entry.name);
      if (entry.isDirectory()) walk(full);
      else files.push(full);
    }
  };
  walk(dir);
  const lower = (p) => path.basename(p).toLowerCase();
  if (binExt === ".exe") {
    return (
      files.find((p) => lower(p) === "mihomo.exe") ||
      files.find((p) => lower(p).startsWith("mihomo") && lower(p).endsWith(".exe"))
    );
  }
  return (
    files.find((p) => lower(p) === "mihomo") ||
    files.find((p) => lower(p).startsWith("mihomo") && !lower(p).includes("."))
  );
}

#!/usr/bin/env node
// 拉取官方签名 wintun.dll → src-tauri/resources/，供 Windows 包内置。
// mihomo TUN 在 Windows 上依赖与 mihomo.exe 同目录的 wintun.dll。
// Author: Corin-Sands
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  renameSync,
  statSync,
  copyFileSync,
  rmSync,
} from "node:fs";
import { unlink } from "node:fs/promises";
import path from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(root, "src-tauri", "resources");
const dest = path.join(outDir, "wintun.dll");
const region = process.env.REGION || "else";
const MIN_BYTES = 100_000;

const target =
  process.argv[2] ||
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.CARGO_BUILD_TARGET ||
  hostTriple();

if (!String(target).includes("windows")) {
  console.log(`非 Windows target (${target})，跳过 wintun`);
  process.exit(0);
}

const archDir = String(target).startsWith("aarch64") ? "arm64" : "amd64";
const ZIP_URL = "https://www.wintun.net/builds/wintun-0.14.1.zip";

mkdirSync(outDir, { recursive: true });
if (existsSync(dest) && statSync(dest).size > MIN_BYTES) {
  console.log(`已有 wintun ${dest} (${statSync(dest).size} bytes)`);
  process.exit(0);
}

const zipPath = path.join(tmpdir(), `wintun-0.14.1-${process.pid}.zip`);
const extractDir = path.join(tmpdir(), `wintun-extract-${process.pid}`);
console.log(`下载 wintun (${archDir}) → ${dest}`);
await download(mirror(ZIP_URL), zipPath);
rmSync(extractDir, { recursive: true, force: true });
mkdirSync(extractDir, { recursive: true });
extractZip(zipPath, extractDir);
await unlink(zipPath).catch(() => {});

const src = path.join(extractDir, "wintun", "bin", archDir, "wintun.dll");
if (!existsSync(src)) {
  console.error(`zip 内无 ${src}`);
  process.exit(1);
}
const size = statSync(src).size;
if (size < MIN_BYTES) {
  console.error(`wintun.dll 过小: ${size}`);
  process.exit(1);
}
const tmp = `${dest}.dl.tmp`;
copyFileSync(src, tmp);
renameSync(tmp, dest);
rmSync(extractDir, { recursive: true, force: true });
console.log(`就绪 ${dest} (${size} bytes)`);

function hostTriple() {
  const { arch, platform } = process;
  if (platform === "win32") {
    return arch === "arm64" ? "aarch64-pc-windows-msvc" : "x86_64-pc-windows-msvc";
  }
  if (platform === "darwin") {
    return arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }
  return arch === "arm64" ? "aarch64-unknown-linux-gnu" : "x86_64-unknown-linux-gnu";
}

function mirror(u) {
  if (region !== "china") return u;
  // wintun.net 国内偶发慢；无稳定镜像时仍直连官方签名包。
  return u;
}

function extractZip(zip, dir) {
  if (process.platform === "win32") {
    execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-Command",
        `Expand-Archive -LiteralPath '${zip.replace(/'/g, "''")}' -DestinationPath '${dir.replace(/'/g, "''")}' -Force`,
      ],
      { stdio: "inherit" },
    );
  } else {
    execFileSync("unzip", ["-o", "-q", zip, "-d", dir], { stdio: "inherit" });
  }
}

async function download(url, file) {
  const res = await fetch(url, {
    headers: { "User-Agent": "supone-tunnel" },
    signal: AbortSignal.timeout(120_000),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${url}`);
  await pipeline(Readable.fromWeb(res.body), createWriteStream(file));
}

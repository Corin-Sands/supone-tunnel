#!/usr/bin/env node
// 拉取 MetaCubeX country.mmdb → src-tauri/resources/，供 bundle.resources 内置。
// 启动时拷到应用数据目录，避免运行时去 GitHub 拉 GeoIP（Windows 等环境常超时）。
// Author: Corin-Sands
import { createWriteStream, existsSync, mkdirSync, renameSync, statSync } from "node:fs";
import { unlink } from "node:fs/promises";
import path from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(root, "src-tauri", "resources");
const dest = path.join(outDir, "country.mmdb");
const region = process.env.REGION || "else";
const MIN_BYTES = 1_000_000;

const SOURCE =
  "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/country.mmdb";

mkdirSync(outDir, { recursive: true });
if (existsSync(dest) && statSync(dest).size > MIN_BYTES) {
  console.log(`已有 geodata ${dest} (${statSync(dest).size} bytes)`);
  process.exit(0);
}

const url = mirror(SOURCE);
const tmp = `${dest}.dl.tmp`;
console.log(`下载 country.mmdb → ${dest}`);
await download(url, tmp);
const size = statSync(tmp).size;
if (size < MIN_BYTES) {
  await unlink(tmp).catch(() => {});
  console.error(`下载过小: ${size} bytes`);
  process.exit(1);
}
renameSync(tmp, dest);
console.log(`就绪 ${dest} (${size} bytes)`);

function mirror(u) {
  if (region !== "china") return u;
  return `https://ghfast.top/${u}`;
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

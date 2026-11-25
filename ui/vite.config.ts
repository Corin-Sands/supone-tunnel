import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import legacy from "@vitejs/plugin-legacy";

/** legacy 会把 polyfill script 插到 head 最前，挤掉 charset；拉回首位防编码误判。Author: Corin-Sands */
function charsetFirst(): Plugin {
  return {
    name: "charset-first",
    transformIndexHtml: {
      // 必须 post：排在 @vitejs/plugin-legacy 注入 polyfill 之后。Author: Corin-Sands
      order: "post",
      handler(html) {
        const meta = '<meta charset="UTF-8" />';
        const stripped = html.replace(
          /\s*<meta\s+charset\s*=\s*["']?UTF-8["']?\s*\/?>\s*/i,
          "\n    ",
        );
        return stripped.replace(/<head([^>]*)>/i, `<head$1>\n    ${meta}`);
      },
    },
  };
}

// Author: Corin-Sands
export default defineConfig({
  plugins: [
    react(),
    // Android-x86 镜像 WebView≈Chrome 75：必须降级 ?. / ?? 等语法。
    legacy({
      targets: ["chrome >= 75"],
      modernPolyfills: true,
    }),
    charsetFirst(),
  ],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: ["es2019", "chrome75"],
    minify: "esbuild",
    outDir: "dist",
  },
});

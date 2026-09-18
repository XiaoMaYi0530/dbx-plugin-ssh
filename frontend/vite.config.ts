import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  // 连接卡品牌标记（PluginLogo）用 ?raw 直接引用仓库根 assets/plugin.svg——
  // 宿主插件中心同款图形、单一来源。frontend 的 vite 根不含该文件，放开 fs
  // 允许到仓库根后 dev server / vitest 的模块解析才能越过 frontend/ 边界
  // （rollup 构建本就不受限）。
  server: { fs: { allow: [fileURLToPath(new URL("../", import.meta.url))] } },
});

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// 与 vite.config.ts 分开：那份带 Tauri 的 build/server 配置，测试用不上，
// 混在一起反而要为 envPrefix、strictPort 之类的东西做无谓的条件分支。
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});

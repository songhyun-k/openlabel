import { defineConfig } from "vitest/config";
import tailwindcss from "@tailwindcss/vite";
export default defineConfig({
  plugins: [tailwindcss()],
  server: { port: 1420, strictPort: true },
  test: { environment: "jsdom", include: ["src/**/*.test.{ts,tsx}"] },
});

import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

// The desktop app loads a fixed local build, so target the WebView2 engine
// directly: no legacy transforms, no polyfills.
export default defineConfig({
  plugins: [preact()],
  clearScreen: false,
  server: { port: 5273, strictPort: true },
  build: { target: "es2022", cssCodeSplit: false, reportCompressedSize: false },
});

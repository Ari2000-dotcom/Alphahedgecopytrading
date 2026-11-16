import { defineConfig, type Plugin } from "vite";
import preact from "@preact/preset-vite";
import fs from "node:fs";
import path from "node:path";

/**
 * Dev only. The desktop window reaches the engine through its Rust host, which
 * holds the engine token. A plain browser (the preview pane, a normal tab) has
 * no host, so while `vite` runs this does the same two jobs on the dev server:
 * the page calls `/__host/<command>` and the token never reaches the page.
 * It is not part of the production build.
 */
function devHost(): Plugin {
  const token = (): string => {
    const env = (process.env.ENGINE_TOKEN ?? "").trim();
    if (env) return env;
    const file = path.join(process.env.APPDATA ?? "", "AlphaHedge Desk", "engine-token.txt");
    try {
      return fs.readFileSync(file, "utf8").split(/\r?\n/)[0].trim();
    } catch {
      throw new Error(`no engine token: set ENGINE_TOKEN, or save it as the first line of ${file}`);
    }
  };
  // The same rules as the Rust host: this machine over http, anything else only over https.
  const allowedUrl = (u: string) =>
    u.startsWith("https://") || ["http://127.0.0.1", "http://localhost", "http://[::1]"].some((p) => u.startsWith(p) && /^(:\d+)?(\/.*)?$/.test(u.slice(p.length)));
  // Only the accounts routes, and only plain ids in the path.
  const allowedPath = (p: string) => /^\/accounts(\/[A-Za-z0-9_-]{1,40}){0,2}$/.test(p);

  return {
    name: "alphahedge-dev-host",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__host", async (req, res) => {
        const send = (code: number, body: unknown) => {
          res.statusCode = code;
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify(body));
        };
        try {
          const cmd = (req.url ?? "").replace(/^\//, "").split("?")[0];
          let raw = "";
          for await (const chunk of req) raw += chunk;
          const a = raw ? JSON.parse(raw) : {};
          if (!allowedUrl(String(a.url ?? ""))) return send(400, { error: "engine address must be https, or http on this machine" });
          const base = String(a.url).replace(/\/+$/, "");

          if (cmd === "engine_health") {
            const t0 = Date.now();
            const r = await fetch(`${base}/health`, { signal: AbortSignal.timeout(2000), redirect: "manual" });
            const j = (await r.json()) as { broker?: string };
            return send(200, { broker: j.broker ?? "unknown", latency_ms: Date.now() - t0 });
          }
          if (cmd === "accounts_call") {
            if (!allowedPath(String(a.path))) return send(400, { error: "not an accounts route" });
            if (!["GET", "PUT", "POST", "DELETE"].includes(a.method)) return send(400, { error: "unsupported method" });
            const r = await fetch(`${base}${a.path}`, {
              method: a.method,
              headers: { authorization: `Bearer ${token()}`, ...(a.body ? { "content-type": "application/json" } : {}) },
              body: a.body ? JSON.stringify(a.body) : undefined,
              signal: AbortSignal.timeout(60000),
              redirect: "manual",
            });
            const j = (await r.json().catch(() => ({}))) as { message?: string; error?: string };
            if (r.ok) return send(200, j);
            if (r.status === 401) return send(502, { error: "the engine refused the token: it does not match the engine's ENGINE_TOKEN" });
            return send(502, { error: j.message ?? j.error ?? "the engine refused the request" });
          }
          return send(404, { error: "unknown command" });
        } catch (e) {
          const m = e instanceof Error ? e.message : String(e);
          return send(502, { error: m.startsWith("no engine token") ? m : "engine not reachable" });
        }
      });
    },
  };
}

// The desktop app loads a fixed local build, so target the WebView2 engine
// directly: no legacy transforms, no polyfills.
export default defineConfig({
  plugins: [preact(), devHost()],
  clearScreen: false,
  server: { port: 5273, strictPort: true },
  build: { target: "es2022", cssCodeSplit: false, reportCompressedSize: false },
});

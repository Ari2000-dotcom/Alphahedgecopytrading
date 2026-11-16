# AlphaHedge Desk

A small, fast Windows desktop screen for `kite-engine` (`E:\kite-engine`). Tauri 2 (system WebView2, no bundled browser) with a Preact front end. It holds no trading logic: the engine does that.

## Run

```bash
npm install
npx tauri dev            # dev window, hot reload on port 5273
npx tauri build --no-bundle   # release exe: src-tauri/target/release/alphahedge-desk.exe
npm run build            # typecheck + front end only
```

The Accounts screen reads and saves accounts through the engine (`/accounts`). The host needs the engine token: set `ENGINE_TOKEN`, or put it as the first line of `%APPDATA%\AlphaHedge Desk\engine-token.txt`. "Save and sign in" saves the details and signs the account in at once. The header shows the engine's state (`/health`, every 5 s while the window is visible) and PAPER or LIVE from the engine's broker.

## Why it is light

- System WebView2 instead of a bundled Chromium; one window; the window starts hidden and shows after the first paint.
- Preact + signals (about 38 kB of JS, 5.5 kB of CSS); system fonts; inline SVG icons; no UI library.
- The Rust host is a 4 MB exe built for size (`opt-level = "s"`, LTO, `panic = "abort"`).
- The page polls nothing while hidden.

## Security

- The host, not the page, calls the engine, so tokens will never be in page JavaScript and the page needs no CORS. It only accepts `https` addresses, or `http` on this machine.
- Secrets are write-only in the UI: saved ones show as "saved" and are never sent back to the screen. Typed secrets stay in the open form only.
- Strict CSP; the window has only the permissions it needs.

## Not built yet

Trade, Orders, Positions, Risk and Alerts screens; the token in the Windows credential store; the kill switch (left out on purpose until it works: a button that does nothing is worse than none).

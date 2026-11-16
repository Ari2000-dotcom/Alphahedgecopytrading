# AlphaHedge copy trading

A multi-broker order-execution backend and a small desktop screen for it, built for Indian F&O trading. **Work in progress. Not production software. Nothing here has been run against a real broker account.**

| Folder | What it is |
|---|---|
| [`kite-engine/`](kite-engine) | Rust backend for Zerodha Kite Connect: place, modify, cancel and read orders behind a token-protected REST API. Entry orders get their stop-loss placed from the real fill price, with partial-fill handling, retries that cannot create a second SL, and a reconciler. **Paper mode is the default.** |
| [`alphahedge-desk/`](alphahedge-desk) | Windows desktop app (Tauri 2, Preact, TypeScript). Accounts screen with per-account sign-in, risk limits, sizing and alert settings, modelled on STOXXO's user settings. Dark, light and match-Windows themes. Currently shows **sample data** and saves nothing. |

## Status

Built and tested:

- Engine: 48 tests against a paper broker and a local mock of Kite's API.
- Desktop app: 23 unit tests, plus checks in a browser. Warm start about 0.5 s, about 165 MB of memory across the app and its web view.

Not built yet:

- The engine's accounts store (encrypted secrets, sign-in test), risk module and kill switch.
- Persisting legs across a restart.
- Trade, Orders, Positions, Risk and Alerts screens.
- Any broker other than Zerodha.

## Safety

- The engine sends no real order unless it is started with `KITE_LIVE=1` plus API keys.
- Every engine route needs a bearer token. The desktop app's host process, not the web page, will call the engine.
- Secrets are write-only in the interface and are never meant to be sent back to the screen.
- No credentials are stored in this repository. Keys come from environment variables.
- Check each broker's current rules on static IPs, API orders and automatic sign-in before using real money.

## Run

Engine, in paper mode:

```bash
cd kite-engine
ENGINE_TOKEN=any-secret-of-16-or-more-chars cargo run
```

Desktop app:

```bash
cd alphahedge-desk
npm install
npx tauri dev
```

See each folder's README for more.

## Licenses

No license has been chosen for this code yet. The Inter font files in `alphahedge-desk/src/assets/fonts` are under the SIL Open Font License (see `Inter-LICENSE.txt` there).

# kite-engine

A small Rust backend for Zerodha Kite Connect: place, modify, cancel and read orders, plus **entry orders whose stop-loss is placed automatically from the real fill price**. Token-protected REST API. **Paper mode is the default**: no real order is sent unless you ask for live mode.

## Run (paper mode)

```bash
ENGINE_TOKEN=any-secret-of-16-or-more-chars cargo run
```

Listens on `127.0.0.1:8088`. Every route needs `Authorization: Bearer <ENGINE_TOKEN>` except `/health` and `/postback/zerodha` (which is checked by Zerodha's checksum).

## Live mode (real money)

Needs all of these, so it cannot start by accident:

| Variable | Meaning |
|---|---|
| `KITE_LIVE=1` | turns live mode on |
| `KITE_API_KEY`, `KITE_ACCESS_TOKEN` | your Kite Connect keys (the access token expires daily) |
| `KITE_API_SECRET` | optional: turns on `/postback/zerodha` and verifies its checksum |
| `KITE_LOCAL_IP` | optional: send from this local IP (a registered static IP on the VPS) |
| `KITE_PROXY` | optional: `http://user:pass@host:port` static-IP proxy |
| `MAX_ORDER_QTY` | optional: largest quantity per order (default 1800) |
| `ENGINE_BIND` | optional: listen address (default `127.0.0.1:8088`) |

In live mode the engine also opens Kite's order websocket for fill updates.

## Accounts and automatic sign-in (Zerodha)

Accounts are saved in an encrypted file (`ACCOUNTS_FILE`, default `accounts.enc`, AES-256-GCM). The key is `ACCOUNTS_KEY`, or the `ENGINE_TOKEN` if that is not set: keep it, or the accounts must be entered again. Secrets are write-only: the API says which are saved, never their values. Access tokens are kept in memory only.

| Method | Path | Does |
|---|---|---|
| GET | `/accounts`, `/accounts/{id}` | accounts and their sign-in state |
| PUT | `/accounts/{id}` | create or update; a blank secret keeps the saved one |
| DELETE | `/accounts/{id}` | remove |
| POST | `/accounts/{id}/login` | sign in now; the error says why it failed |
| POST | `/accounts/login-all` | sign in every account marked auto-login, all at once |

Sign-in details per broker (secret keys are write-only; the others are plain fields):

| Broker | Needs | Notes |
|---|---|---|
| Zerodha | user ID; `apiKey`, `apiSecret`, `password`, `totpSecret` | drives Kite's web sign-in; the first connection of an app needs one click on Kite's "Authorize" page |
| Angel One | client code; `apiKey`, `pin`, `totpSecret` | one TOTP call; also reads available cash |
| Kotak Neo | UCC; `apiKey` (consumer key), `pin` (MPIN), `totpSecret`; field `mobile` | two steps, ends with the trade token and this account's data-centre host |
| Upstox | `apiKey`, `apiSecret`, `pin`, `totpSecret`; fields `mobile`, `redirectUrl` | drives Upstox's sign-in; the account must use TOTP, not SMS, and the redirect URL must match the console exactly |
| Nubra | `pin` (MPIN), `totpSecret`; field `mobile` | the TOTP is made from Nubra's own clock |

The logins are ported from the working adapters in `D:\Alphahedgedeskackendust\srcrokersdapters`. Zerodha and Upstox have no documented login call, so they copy what the broker's own page does and can break when the page changes; the error names the step.

**Nothing signs in by itself.** Sign-in happens only when asked: `POST /accounts/{id}/login` for one account, or `POST /accounts/login-all` for every account with `auto_login` ticked, all at once (one result per account, so one wrong password does not hide the others). Each sign-in returns the account's funds where the broker gives them cheaply (Zerodha, Angel One and Upstox so far). A TOTP code is never made in the last 3 s of its 30 s window. Signing in does not yet route orders to that account: orders still use the single broker configured by the environment.

## Entry + SL (a "leg")

```bash
curl -X POST localhost:8088/legs -H "Authorization: Bearer $ENGINE_TOKEN" -H 'content-type: application/json' -d '{
  "leg_id": "DEMO1",
  "entry": {"exchange":"NFO","tradingsymbol":"NIFTY2611224500CE","side":"SELL",
            "order_type":"LIMIT","product":"NRML","quantity":75,"price":100.0},
  "sl": {"percent": 50}
}'
```

1. The entry is placed with tag `DEMO1`.
2. When a fill is seen, the SL is placed for the **filled quantity** at the **actual average price**: a sold option gets a BUY SL at `avg x (1 + percent)`, a bought one gets a SELL SL at `avg x (1 - percent)`, rounded to the 0.05 tick in the safe direction.
3. More fills: the SL quantity and trigger are modified. It never exceeds what is filled.
4. SL fills: the leg closes.

`sl` options: `percent` (of the fill price), `kind` (`SL` stop-limit, default, or `SL-M`), `limit_buffer_pct` (default 5: the SL limit sits this far beyond the trigger), `tick` (0.05), `on_failure` (`exit_at_market`, default, or `alert_only`).

**If the SL cannot be placed:** network-type failures are retried (after checking the broker has no SL with that tag, so a lost reply never makes a second SL). If the broker refuses, the leg is marked `unprotected`, an alert is raised, and with `exit_at_market` the filled quantity is closed at market.

| Method | Path | Does |
|---|---|---|
| POST | `/legs` | place an entry with its SL rule |
| GET | `/legs`, `/legs/{id}` | state, fills, SL levels, event log, fill-to-SL timing |
| PUT | `/legs/{id}/sl` | change the SL percent; moves the SL at the broker |
| POST | `/legs/{id}/exit` | cancel the SL and close at market |
| POST | `/paper/fill` | **paper mode only**: fill (or part-fill) a resting order by hand |
| POST | `/postback/zerodha` | Zerodha's postback; needs `KITE_API_SECRET` |

Fills are detected three ways, all feeding one idempotent handler: the **order websocket**, the **postback**, and a **reconciler** that reads the order book every 2 s (and after every reconnect). A read-back shortly after placing the entry covers a late push.

## Plain orders

| Method | Path | Does |
|---|---|---|
| GET | `/health` | liveness, which broker |
| POST | `/orders` | place an order |
| PUT | `/orders/{variety}/{id}` | modify an open order |
| DELETE | `/orders/{variety}/{id}` | cancel an open order |
| GET | `/orders`, `/orders/{id}` | order book, one order's history |
| GET | `/positions`, `/margins` | positions, funds |

Errors come back as `{"error": "<kind>", "message": "..."}`: `validation` (400), `risk` (422), `duplicate` (409), `not_found` (404), `broker_rejected` / `network` / `bad_response` (502), `outcome_unknown` (504).
**`outcome_unknown` means the order may have reached the broker. Read `/orders` before retrying.**

## Not done yet (read before going live)

- **Leg state is in memory.** A restart forgets legs (the SLs stay at the broker). Next step: persist legs (SQLite) and rebuild from the broker's order book by tag.
- **Tested only against paper and local mocks.** Nothing here has talked to the real Zerodha. Check on a 1-lot order first: whether Zerodha accepts `SL` (and `SL-M`) on your option, the real push payload for partial fills, and the percent meaning.
- No kill switch, no Telegram alerts, one broker account, no trailing or target yet.
- The emergency market exit sends no `market_protection`; set it if Zerodha requires it.

## Layout

```
src/types.rs    request and response shapes, validation, tick rounding
src/broker.rs   the Broker trait every broker implements
src/kite.rs     Zerodha client (static IP / proxy options)
src/paper.rs    in-memory broker, with fault injection for tests
src/engine.rs   validation + risk limits + duplicate guard
src/leg.rs      entry + SL state machine, reconciler
src/ws.rs       Kite order websocket (reconnect, dead-link detection)
src/api.rs      REST routes, token check, postback checksum
tests/          mock-Kite, API and leg tests (cargo test)
```

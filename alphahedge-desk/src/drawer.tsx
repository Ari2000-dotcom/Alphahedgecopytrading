import { useState } from "preact/hooks";
import type { ComponentChildren } from "preact";
import { BROKERS, openId, OUTBOUND, showToast, updateAccount, type Account, type BrokerId } from "./store";
import { checkLimits, normalizeSqOff, num, validEmail, validTelegram, validTime, type Parsed } from "./validate";

type Tab = "signin" | "risk" | "sizing" | "alerts";
const TABS: { id: Tab; label: string }[] = [
  { id: "signin", label: "Sign-in" },
  { id: "risk", label: "Risk limits" },
  { id: "sizing", label: "Sizing" },
  { id: "alerts", label: "Alerts" },
];

interface Form {
  broker: BrokerId;
  vals: Record<string, string>;
  auto: boolean;
  outbound: string;
  slPct: string;
  slFailure: "exit" | "alert";
  qtyMultiplier: string;
  exposure: string;
  maxProfit: string;
  maxLoss: string;
  lockAt: string;
  lockMin: string;
  trailEvery: string;
  trailBy: string;
  maxOpenTrades: string;
  maxLossPerTrade: string;
  qtyOnMaxLoss: boolean;
  sqOffTime: string;
  sqOffNrml: boolean;
  telegramIds: string;
  email: string;
  authRequired: boolean;
  historicalApi: boolean;
}

// Zero means "off", so show it as a blank box.
const show = (n: number) => (n ? String(n) : "");

function init(a: Account): Form {
  return {
    broker: a.broker,
    // Typed secrets live only in this form while it is open. They are never
    // kept on the account and are gone when the drawer closes.
    vals: { userId: a.userId, ...a.fields },
    auto: a.autoLogin, outbound: a.outbound, slPct: String(a.slPct), slFailure: a.slFailure,
    qtyMultiplier: String(a.qtyMultiplier), exposure: show(a.exposure),
    maxProfit: show(a.risk.maxProfit), maxLoss: show(a.risk.maxLoss),
    lockAt: show(a.risk.lockAt), lockMin: show(a.risk.lockMin),
    trailEvery: show(a.risk.trailEvery), trailBy: show(a.risk.trailBy),
    maxOpenTrades: show(a.risk.maxOpenTrades), maxLossPerTrade: show(a.risk.maxLossPerTrade),
    qtyOnMaxLoss: a.risk.qtyOnMaxLoss, sqOffTime: a.risk.sqOffTime, sqOffNrml: a.risk.sqOffNrml,
    telegramIds: a.alerts.telegramIds, email: a.alerts.email, authRequired: a.alerts.authRequired,
    historicalApi: a.historicalApi,
  };
}

function Field({ label, hint, wide, children }: { label: ComponentChildren; hint?: string; wide?: boolean; children: ComponentChildren }) {
  return (
    <label class={"fld" + (wide ? " wide" : "")}>
      <span class="lab">{label}</span>
      {children}
      {hint && <small>{hint}</small>}
    </label>
  );
}

function Money({ label, hint, value, set }: { label: string; hint?: string; value: string; set: (v: string) => void }) {
  return (
    <Field label={label} hint={hint}>
      <span class="affix">
        <i>₹</i>
        <input inputMode="decimal" autocomplete="off" placeholder="Off" value={value} onInput={(e) => set(e.currentTarget.value)} />
      </span>
    </Field>
  );
}

function Check({ checked, set, children }: { checked: boolean; set: (v: boolean) => void; children: ComponentChildren }) {
  return (
    <label class="chk">
      <input type="checkbox" checked={checked} onChange={(e) => set(e.currentTarget.checked)} />
      <span>{children}</span>
    </label>
  );
}

const Group = ({ title, children }: { title: string; children: ComponentChildren }) => (
  <section class="group">
    <h3>{title}</h3>
    <div class="cols">{children}</div>
  </section>
);

const NOT_ENFORCED = "Saved with the account. The engine starts enforcing this once its risk module is built.";

export function Drawer({ a }: { a: Account }) {
  const [f, setF] = useState<Form>(() => init(a));
  const [tab, setTab] = useState<Tab>("signin");
  const [err, setErr] = useState("");
  const def = BROKERS[f.broker];
  const locked = Object.keys(a.saved).length > 0;
  const put = <K extends keyof Form>(k: K) => (v: Form[K]) => setF((p) => ({ ...p, [k]: v }));

  function fail(t: Tab, msg: string) {
    setTab(t);
    setErr(msg);
  }

  function save() {
    const money = (label: string, s: string) => num(label, s);
    const checks: [Tab, Parsed<number>][] = [
      ["signin", num("Default SL", f.slPct, { required: true, positive: true, max: 99.99 })],
      ["sizing", num("Quantity multiplier", f.qtyMultiplier, { required: true, positive: true })],
      ["sizing", money("Exposure per trade", f.exposure)],
      ["risk", money("Max profit", f.maxProfit)], ["risk", money("Max loss", f.maxLoss)],
      ["risk", money("Profit that triggers the lock", f.lockAt)], ["risk", money("Profit to lock", f.lockMin)],
      ["risk", money("Profit increase to trail on", f.trailEvery)], ["risk", money("Trail by", f.trailBy)],
      ["risk", num("Max open trades", f.maxOpenTrades, { int: true })],
      ["risk", money("Max loss per trade", f.maxLossPerTrade)],
    ];
    for (const [t, p] of checks) if (!p.ok) return fail(t, p.error);
    const v = checks.map(([, p]) => (p.ok ? p.value : 0));
    const [slPct, qtyMultiplier, exposure, maxProfit, maxLoss, lockAt, lockMin, trailEvery, trailBy, maxOpenTrades, maxLossPerTrade] = v;

    const missing = def.fields.filter((x) => !x.secret && !x.label.includes("optional") && !(f.vals[x.key] ?? "").trim());
    if (missing.length) return fail("signin", `Enter ${missing[0].label}`);
    if (!validTime(f.sqOffTime)) return fail("risk", "Square-off time must look like 15:15 or 15:15:00");
    if (!validTelegram(f.telegramIds)) return fail("alerts", "Telegram IDs are numbers separated by commas");
    if (!validEmail(f.email)) return fail("alerts", "Enter a valid email address, like name@company.com");
    const rule = checkLimits({ maxProfit, maxLoss, lockAt, lockMin, trailEvery, trailBy, maxLossPerTrade, qtyOnMaxLoss: f.qtyOnMaxLoss, authRequired: f.authRequired, telegramIds: f.telegramIds });
    if (rule) return fail(/Telegram/.test(rule) ? "alerts" : "risk", rule);

    const saved = { ...a.saved };
    const fields: Record<string, string> = {};
    for (const x of def.fields) {
      if (x.secret && (f.vals[x.key] ?? "").trim()) saved[x.key] = true;
      if (!x.secret && x.key !== "userId") fields[x.key] = (f.vals[x.key] ?? "").trim();
    }
    updateAccount(a.id, {
      broker: f.broker, slPct, slFailure: f.slFailure, autoLogin: f.auto, outbound: f.outbound, historicalApi: f.historicalApi,
      qtyMultiplier, exposure, saved, fields, userId: (f.vals.userId ?? a.userId).trim() || a.userId,
      risk: {
        maxProfit, maxLoss, lockAt, lockMin, trailEvery, trailBy, maxOpenTrades, maxLossPerTrade,
        qtyOnMaxLoss: f.qtyOnMaxLoss, sqOffTime: normalizeSqOff(f.sqOffTime), sqOffNrml: f.sqOffNrml,
      },
      alerts: { telegramIds: f.telegramIds.trim(), email: f.email.trim(), authRequired: f.authRequired },
    });
    // Typed secrets are dropped from the form the moment they are saved.
    setF((p) => ({ ...p, vals: { userId: p.vals.userId ?? "", ...fields } }));
    setErr("");
    showToast("Saved. The sign-in test arrives with the engine's accounts store");
  }

  function onTabKey(e: KeyboardEvent) {
    const i = TABS.findIndex((t) => t.id === tab);
    const n = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : -1;
    if (n < 0) return;
    e.preventDefault();
    const next = TABS[(n + TABS.length) % TABS.length].id;
    setTab(next);
    document.getElementById("tab-" + next)?.focus();
  }

  return (
    <div class="drawer">
      <div class="dhead">
        <h2>{a.alias}<span class="sub"> · {BROKERS[a.broker].name}</span></h2>
        <span class="grow" />
        <span class="sub">Saved secrets are never shown again</span>
      </div>

      <div class="tabs" role="tablist" aria-label="Account settings" onKeyDown={onTabKey}>
        {TABS.map((t) => (
          <button
            key={t.id} id={"tab-" + t.id} role="tab" aria-selected={tab === t.id} aria-controls="panel"
            tabIndex={tab === t.id ? 0 : -1} class={"tab" + (tab === t.id ? " on" : "")}
            onClick={() => { setTab(t.id); setErr(""); }}
          >{t.label}</button>
        ))}
      </div>

      <div id="panel" role="tabpanel" aria-labelledby={"tab-" + tab}>
        {tab === "signin" && (
          <>
            {!locked && (
              <Group title="Broker">
                <Field label="Broker">
                  <select value={f.broker} onChange={(e) => { put("broker")(e.currentTarget.value as BrokerId); setErr(""); }}>
                    {(Object.keys(BROKERS) as BrokerId[]).map((b) => <option key={b} value={b}>{BROKERS[b].name}</option>)}
                  </select>
                </Field>
              </Group>
            )}
            {def.fields.length === 0 ? (
              <p class="note">{def.note}</p>
            ) : (
              <>
                <Group title="Credentials">
                  {def.fields.map((x) => {
                    const isSaved = x.secret && a.saved[x.key];
                    const common = {
                      autocomplete: "off", spellcheck: false,
                      placeholder: isSaved ? "Saved · type to replace" : "",
                      value: f.vals[x.key] ?? "",
                      onInput: (e: { currentTarget: HTMLInputElement }) => setF((p) => ({ ...p, vals: { ...p.vals, [x.key]: e.currentTarget.value } })),
                    };
                    return (
                      <Field key={f.broker + x.key} hint={x.hint} label={<>{x.label}{isSaved && <em class="saved"> · saved</em>}</>}>
                        {x.secret ? <input type="password" {...common} /> : <input type="text" {...common} />}
                      </Field>
                    );
                  })}
                </Group>
                <Group title="Sign-in and orders">
                  <Field label="Orders leave from">
                    <select value={f.outbound} onChange={(e) => put("outbound")(e.currentTarget.value)}>
                      {OUTBOUND.map((o) => <option key={o}>{o}</option>)}
                    </select>
                  </Field>
                  <Field label="Default stop-loss" hint="Percent of the fill price">
                    <span class="affix"><input inputMode="decimal" value={f.slPct} onInput={(e) => put("slPct")(e.currentTarget.value)} /><i class="post">%</i></span>
                  </Field>
                  <Field label="If a stop-loss can't be placed">
                    <select value={f.slFailure} onChange={(e) => put("slFailure")(e.currentTarget.value as "exit" | "alert")}>
                      <option value="exit">Close the position at market</option>
                      <option value="alert">Only alert me</option>
                    </select>
                  </Field>
                </Group>
                <div class="checks">
                  <Check checked={f.auto} set={put("auto")}>Sign in automatically every morning</Check>
                  <p class="hint">Needs the password, PIN and TOTP secret. If automatic sign-in fails, use "Sign in again" on the row to sign in by hand.</p>
                  <Check checked={f.historicalApi} set={put("historicalApi")}>The broker's historical (candle) API is available on this account</Check>
                  <p class="hint">Used to read candle data from the broker. If you don't know, leave it off.</p>
                </div>
                <p class="note">{def.note}</p>
              </>
            )}
          </>
        )}

        {tab === "risk" && (
          <>
            <p class="note">{NOT_ENFORCED} Leave a box empty to switch that limit off.</p>
            <Group title="Daily limits">
              <Money label="Max profit" value={f.maxProfit} set={put("maxProfit")} hint="Close everything and stop trading on this account when MTM reaches this. Counts all intraday positions, including ones entered by hand" />
              <Money label="Max loss" value={f.maxLoss} set={put("maxLoss")} hint="Close everything and stop trading on this account when MTM falls to this. Counts all intraday positions, including ones entered by hand" />
              <Field label="Square-off time" hint="Closes all positions, including ones entered by hand. After this time no strategy can place orders on this account. Empty or 00:00:00 means off">
                <input placeholder="Off" value={f.sqOffTime} onInput={(e) => put("sqOffTime")(e.currentTarget.value)} />
              </Field>
            </Group>
            <div class="checks">
              <Check checked={f.sqOffNrml} set={put("sqOffNrml")}>Also close positional (NRML) positions at square-off</Check>
              <p class="hint">Use with care: this also closes positions you carry overnight, not only today's.</p>
            </div>
            <Group title="Trade limits">
              <Field label="Max open trades" hint="No new orders until one is closed">
                <input inputMode="numeric" placeholder="Off" value={f.maxOpenTrades} onInput={(e) => put("maxOpenTrades")(e.currentTarget.value)} />
              </Field>
              <Money label="Max loss per trade" value={f.maxLossPerTrade} set={put("maxLossPerTrade")} hint="Exit a trade that loses more than this. If a symbol sets its own, the lower of the two applies" />
            </Group>
            <div class="checks">
              <Check checked={f.qtyOnMaxLoss} set={put("qtyOnMaxLoss")}>Size the quantity from max loss per trade and the stop-loss</Check>
              <p class="hint">Example: a max loss per trade of 500 with a 5-point stop-loss gives a quantity of 100. With no stop-loss, the amount only decides when to exit a losing trade.</p>
            </div>
            <Group title="Lock and trail profit">
              <Money label="If profit reaches" value={f.lockAt} set={put("lockAt")} hint="Example: if profit reaches 1,000, lock in at least 500. It also works as a break-even point" />
              <Money label="Lock minimum profit at" value={f.lockMin} set={put("lockMin")} />
              <Money label="For every profit increase of" value={f.trailEvery} set={put("trailEvery")} hint="Example: for every 1,000 more profit, trail by 500. Keep some gap between the two amounts" />
              <Money label="Trail profit by" value={f.trailBy} set={put("trailBy")} />
            </Group>
          </>
        )}

        {tab === "sizing" && (
          <>
            <p class="note">{NOT_ENFORCED}</p>
            <Group title="Quantity">
              <Field label="Quantity multiplier" hint="A quantity of 100 with 2 becomes 200, and with 0.5 becomes 50. Rounded to whole lots">
                <input inputMode="decimal" value={f.qtyMultiplier} onInput={(e) => put("qtyMultiplier")(e.currentTarget.value)} />
              </Field>
              <Money label="Exposure per trade" value={f.exposure} set={put("exposure")} hint="Quantity = this amount ÷ price, rounded to the nearest lot (100 with a lot of 75 becomes 75, and 120 becomes 150). If no price is available the last close is used. Takes priority over other quantity settings except a symbol's max quantity" />
            </Group>
          </>
        )}

        {tab === "alerts" && (
          <>
            <p class="note">{NOT_ENFORCED}</p>
            <Group title="Telegram">
              <Field wide label="Telegram IDs" hint="Numeric IDs from the bot, separated by commas. Groups and channels start with a minus sign. The Telegram bot has to be set up first">
                <input placeholder="12001200, -412554122" value={f.telegramIds} onInput={(e) => put("telegramIds")(e.currentTarget.value)} />
              </Field>
              <Field wide label="Email" hint="Email alerts aren't available yet. The address is saved for when they are">
                <input type="text" inputMode="email" autocomplete="off" placeholder="name@company.com" value={f.email} onInput={(e) => put("email")(e.currentTarget.value)} />
              </Field>
            </Group>
            <div class="checks">
              <Check checked={f.authRequired} set={put("authRequired")}>Ask for approval on Telegram before trades</Check>
              <p class="hint">The client can approve the whole day or each trade from the bot, and needs a Telegram ID above. A request with no answer in 10 minutes counts as declined, so orders can be delayed.</p>
            </div>
          </>
        )}
      </div>

      <div class="dfoot">
        <button class="btn primary" onClick={save}>Save and test sign-in</button>
        <button class="btn" onClick={() => (openId.value = null)}>Close</button>
        {err && <span class="err" role="alert">{err}</span>}
      </div>
    </div>
  );
}

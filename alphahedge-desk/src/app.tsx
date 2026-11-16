import { useEffect, useRef } from "preact/hooks";
import { signal } from "@preact/signals";
import { invoke } from "./host";
import {
  accounts, accountsLoaded, addAccount, BROKERS, busyAll, busyFunds, CRED_COLS, currentVal, dropDraft, drafts, engine, engineUrl, fieldFor,
  filters, isDirty, newAccount, patchDraft, removeAccount, revealed, revealLeft, revealTyped, setEnabled, setPersist, setTheme, showToast, sortState, theme, toast, updateAccount,
  type Account, type BrokerId, type Theme,
} from "./store";
import { filtersActive, nextSort, noFilters, viewRows, type SortKey } from "./grid";
import { deleteAccount, fetchSecrets, loadAccounts, refreshFunds, saveAccount, signIn, signInAll } from "./api";
import { askConfirm, ConfirmHost } from "./confirm";
import { moneyParts } from "./format";
import { passes, SECRET_COLS, type FilterRow } from "./colfilter";
import * as I from "./icons";

// ------------------------------------------------------------------ engine

async function ping() {
  // Nothing to do while the window is hidden or minimised.
  if (document.visibilityState !== "visible") return;
  try {
    const r = await invoke<{ broker: string; latency_ms: number }>("engine_health", { url: engineUrl.value });
    engine.value = { state: "up", broker: r.broker, ms: r.latency_ms };
  } catch {
    engine.value = { state: "down" };
  }
}

async function refresh() {
  if (document.visibilityState !== "visible") return;
  try {
    await loadAccounts();
    accountsLoaded.value = true;
  } catch (e) {
    // Said once, while nothing has loaded; the header already shows the engine's state.
    if (!accountsLoaded.value) showToast(String(e));
  }
}

/** Reads margins again, quietly, when at least one account is signed in. */
async function silentFunds() {
  if (document.visibilityState !== "visible" || busyFunds.value) return;
  if (!accounts.value.some((a) => a.status === "in")) return;
  try {
    await refreshFunds();
  } catch {
    /* the Refresh margins button reports errors; a quiet poll does not nag */
  }
}

function useEngine() {
  useEffect(() => {
    setPersist(saveAccount);
    ping();
    refresh();
    const t = window.setInterval(ping, 5000);
    const r = window.setInterval(refresh, 30000);
    const f = window.setInterval(silentFunds, 60000);
    const onVisible = () => { ping(); refresh(); };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(t);
      clearInterval(r);
      clearInterval(f);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);
}

// ------------------------------------------------------------------ shell

function Header() {
  const e = engine.value;
  const mode = e.state === "up" ? (e.broker === "paper" ? "Paper" : "Live") : "Offline";
  return (
    <header class="top">
      <div class="brand">
        <span class="mark"><I.Mark /></span>
        <b>AlphaHedge Desk</b>
      </div>
      <span class={"badge " + mode.toLowerCase()}>{mode}</span>
      <span class="grow" />
      <ThemeSwitch />
      <span class="status">
        <i class={"dot " + (e.state === "up" ? "ok" : e.state === "down" ? "bad" : "mid")} />
        {e.state === "up" ? `Engine connected · ${e.ms} ms` : e.state === "down" ? "Engine not running" : "Checking engine…"}
      </span>
    </header>
  );
}

interface NavEntry { label: string; icon: () => preact.JSX.Element; live?: boolean }

/** Trading screens, then the screens that manage the desk, then settings at the foot. */
const NAV_TRADING: NavEntry[] = [
  { label: "Trade", icon: I.NavTrade },
  { label: "Orders", icon: I.NavOrders },
  { label: "Positions", icon: I.NavPositions },
];
const NAV_DESK: NavEntry[] = [
  { label: "Accounts", icon: I.NavAccounts, live: true },
  { label: "Risk", icon: I.NavRisk },
  { label: "Alerts", icon: I.NavAlerts },
];
const NAV_FOOT: NavEntry[] = [{ label: "Settings", icon: I.NavSettings }];

function NavItem({ n }: { n: NavEntry }) {
  return (
    <button
      class={"navbtn" + (n.live ? " on" : " soon")}
      aria-label={n.live ? n.label : `${n.label}, not built yet`}
      aria-current={n.live ? "page" : undefined}
      onClick={() => !n.live && showToast(`${n.label} is not built yet`)}
    >
      <n.icon />
      <span class="tip" role="tooltip">{n.label}{!n.live && <em>Soon</em>}</span>
    </button>
  );
}

function Nav() {
  return (
    <nav class="nav" aria-label="Sections">
      <div class="navgroup">{NAV_TRADING.map((n) => <NavItem key={n.label} n={n} />)}</div>
      <span class="navsep" />
      <div class="navgroup">{NAV_DESK.map((n) => <NavItem key={n.label} n={n} />)}</div>
      <span class="grow" />
      <div class="navgroup">{NAV_FOOT.map((n) => <NavItem key={n.label} n={n} />)}</div>
    </nav>
  );
}

const THEMES: { id: Theme; label: string; icon: () => preact.JSX.Element }[] = [
  { id: "dark", label: "Dark", icon: I.Moon },
  { id: "light", label: "Light", icon: I.Sun },
  { id: "system", label: "Match Windows", icon: I.Monitor },
];

function ThemeSwitch() {
  return (
    <div class="seg" role="group" aria-label="Colour theme">
      {THEMES.map((t) => (
        <button key={t.id} class={theme.value === t.id ? "on" : ""} aria-pressed={theme.value === t.id} title={t.label} aria-label={t.label} onClick={() => setTheme(t.id)}>
          <t.icon />
        </button>
      ))}
    </div>
  );
}

// ------------------------------------------------------------------- numbers

/** Money the way a broker shows it: grouped, two decimals, dimmed decimals.
 *  A figure that changes flashes once, so a refresh is visible. */
function Money({ n }: { n: number }) {
  const ref = useRef<HTMLSpanElement>(null);
  const prev = useRef(n);
  useEffect(() => {
    const el = ref.current;
    if (el && prev.current !== n) {
      el.classList.remove("flash");
      void el.offsetWidth;
      el.classList.add("flash");
    }
    prev.current = n;
  }, [n]);
  const p = moneyParts(n, false);
  if (!p) return <span ref={ref} class="money zero">–</span>;
  return <span ref={ref} class="money">{p.sign}{p.int}<span class="frac">{p.frac}</span></span>;
}

function Figures({ list }: { list: Account[] }) {
  const real = list.filter((a) => a.on && a.broker !== "paper");
  const sum = (pick: (f: NonNullable<Account["funds"]>) => number) => real.reduce((t, a) => t + (a.funds ? pick(a.funds) : 0), 0);
  const available = sum((f) => f.available);
  const used = sum((f) => f.used);
  const total = sum((f) => f.total);
  const cash = sum((f) => f.cash ?? 0);
  const collateral = sum((f) => f.collateral);
  const signed = real.filter((a) => a.status === "in").length;
  const usedPct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
  const signedPct = real.length > 0 ? (signed / real.length) * 100 : 0;
  return (
    <div class="figures">
      <div class="fig"><span class="lab">Available margin</span><b class="val"><Money n={available} /></b></div>
      <div class="fig">
        <span class="lab">Used margin{total > 0 ? ` · ${usedPct.toFixed(1)}%` : ""}</span>
        <b class="val"><Money n={used} /></b>
        <div class={"meter" + (usedPct > 80 ? " warn" : "")} role="img" aria-label={`${usedPct.toFixed(0)} percent of margin in use`}>
          <i style={{ transform: `scaleX(${usedPct / 100})` }} />
        </div>
      </div>
      <div class="fig"><span class="lab">Total margin</span><b class="val"><Money n={total} /></b></div>
      <div class="fig"><span class="lab">Cash</span><b class="val"><Money n={cash} /></b></div>
      <div class="fig"><span class="lab">Collateral</span><b class="val"><Money n={collateral} /></b></div>
      <div class="fig">
        <span class="lab">Signed in</span>
        <b class="val">{signed}<span class="of"> of {real.length}</span></b>
        <div class="meter" role="img" aria-label={`${signed} of ${real.length} accounts signed in`}>
          <i style={{ transform: `scaleX(${signedPct / 100})` }} />
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- accounts

const signingIn = signal<Set<string>>(new Set());
const setSigning = (id: string, on: boolean) => {
  const next = new Set(signingIn.value);
  if (on) next.add(id); else next.delete(id);
  signingIn.value = next;
};

/** Changes a row's tick boxes. A new row keeps them until it is saved; a saved row saves at once. */
function patchRow(a: Account, patch: Partial<Account>) {
  if (a.unsaved) newAccount.value = { ...a, ...patch };
  else updateAccount(a.id, patch);
}

const place = (n: Account) => {
  accounts.value = accounts.value.some((x) => x.id === n.id) ? accounts.value.map((x) => (x.id === n.id ? n : x)) : [...accounts.value, n];
};

/** The broker a row is set to right now (a new row can still change it). */
const brokerOf = (a: Account): BrokerId => drafts.value[a.id]?.broker ?? a.broker;

/** Saves what was typed in a row, then signs the account in. */
async function saveRow(a: Account) {
  if (signingIn.value.has(a.id)) return;
  const d = drafts.value[a.id] ?? { vals: {}, invalid: [] };
  const broker = brokerOf(a);
  const alias = (d.alias ?? a.alias).trim();
  const def = BROKERS[broker];
  const typed = (key: string) => (d.vals[key] ?? "").trim();

  const flag = (keys: string[], msg: string) => {
    patchDraft(a.id, {});
    drafts.value = { ...drafts.value, [a.id]: { ...(drafts.value[a.id] ?? { vals: {}, invalid: [] }), invalid: keys } };
    showToast(msg);
  };
  if (!alias) return flag(["alias"], "Enter a name for this account");
  const missing = def.fields.filter((f) => {
    const have = f.secret ? !!typed(f.key) || (!!a.saved[f.key] && broker === a.broker) : !!(d.vals[f.key] !== undefined ? typed(f.key) : currentVal(a, f.key).trim());
    return !have;
  });
  if (missing.length) return flag(missing.map((f) => f.key), `${def.name} needs: ${missing.map((f) => f.label).join(", ")}`);

  const fields: Record<string, string> = {};
  const secrets: Record<string, string> = {};
  let userId = a.userId;
  for (const f of def.fields) {
    const v = d.vals[f.key] !== undefined ? typed(f.key) : f.secret ? "" : currentVal(a, f.key).trim();
    if (f.secret) { if (v) secrets[f.key] = v; }
    else if (f.key === "userId") userId = v;
    else fields[f.key] = v;
  }
  const next: Account = { ...a, alias, broker, userId, fields, saved: broker === a.broker ? a.saved : {} };

  setSigning(a.id, true);
  try {
    let saved: Account;
    try {
      saved = await saveAccount(next, secrets);
    } catch (e) {
      showToast(`Not saved: ${String(e)}`);
      return;
    }
    if (a.unsaved) newAccount.value = null;
    dropDraft(a.id);
    place(saved);
    if (broker === "paper") {
      showToast(`${alias} saved`);
      return;
    }
    try {
      place(await signIn(a.id));
      showToast(`${alias} signed in`);
    } catch (e) {
      // Saved either way. The reason shows under the row.
      place({ ...saved, signInError: String(e) });
      showToast(`${alias}: ${String(e)}`);
    }
  } finally {
    setSigning(a.id, false);
  }
}

function discardRow(a: Account) {
  dropDraft(a.id);
  if (a.unsaved) newAccount.value = null;
}

async function doSignIn(a: Account) {
  if (signingIn.value.has(a.id)) return;
  setSigning(a.id, true);
  try {
    const n = await signIn(a.id);
    place(n);
    showToast(`${a.alias} signed in`);
  } catch (e) {
    await loadAccounts().catch(() => {});
    showToast(`${a.alias}: ${String(e)}`);
  } finally {
    setSigning(a.id, false);
  }
}

async function doLoginAll() {
  if (busyAll.value) return;
  const marked = accounts.value.filter((a) => a.autoLogin && a.broker !== "paper");
  if (marked.length === 0) {
    showToast("Tick Auto on the accounts you want signed in");
    return;
  }
  busyAll.value = true;
  try {
    const r = await signInAll();
    const bad = r.filter((x) => !x.ok);
    const name = (id: string) => accounts.value.find((a) => a.id === id)?.alias ?? id;
    showToast(
      bad.length === 0
        ? `${r.length} of ${r.length} signed in`
        : `${r.length - bad.length} of ${r.length} signed in. ${name(bad[0].id)}: ${bad[0].error ?? "failed"}`,
    );
  } catch (e) {
    showToast(String(e));
  } finally {
    busyAll.value = false;
  }
}

async function doRefreshFunds() {
  if (busyFunds.value) return;
  busyFunds.value = true;
  try {
    await refreshFunds();
    const stale = accounts.value.filter((a) => a.status === "in" && a.fundsError);
    showToast(stale.length === 0 ? "Margins updated" : `${stale[0].alias}: ${stale[0].fundsError}`);
  } catch (e) {
    showToast(String(e));
  } finally {
    busyFunds.value = false;
  }
}

const REVEAL_SECONDS = 60;
let revealTimer: number | undefined;

function hideValues() {
  clearInterval(revealTimer);
  revealTimer = undefined;
  revealTyped.value = false;
  revealed.value = {};
  revealLeft.value = 0;
}

/** Shows every saved value as text for a minute, then hides it again. Also hides when the window is hidden. */
async function toggleValues() {
  if (revealTyped.value) return hideValues();
  const withSaved = accounts.value.filter((a) => Object.keys(a.saved).length > 0);
  try {
    const pairs = await Promise.all(withSaved.map(async (a) => [a.id, await fetchSecrets(a.id)] as const));
    revealed.value = Object.fromEntries(pairs);
  } catch (e) {
    showToast(`Could not read the saved values: ${String(e)}`);
    return;
  }
  revealTyped.value = true;
  revealLeft.value = REVEAL_SECONDS;
  clearInterval(revealTimer);
  revealTimer = window.setInterval(() => {
    revealLeft.value -= 1;
    if (revealLeft.value <= 0) hideValues();
  }, 1000);
}

if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible" && revealTyped.value) hideValues();
  });
}

async function copyValue(label: string, value: string) {
  try {
    await navigator.clipboard.writeText(value);
    showToast(`${label} copied`);
  } catch {
    showToast("Could not copy");
  }
}

function askDelete(a: Account) {
  askConfirm({
    title: `Delete ${a.alias}?`,
    body: [
      `${BROKERS[a.broker].name}${a.userId ? " · " + a.userId : ""}. This removes its saved sign-in details from this app.`,
      "Positions and orders at the broker are not touched.",
    ],
    confirmLabel: "Delete account",
    danger: true,
    onConfirm: async () => {
      try {
        await deleteAccount(a.id);
        removeAccount(a.id);
        showToast(`${a.alias} deleted`);
      } catch (e) {
        showToast(`Not deleted: ${String(e)}`);
      }
    },
  });
}

// ------------------------------------------------------------------ columns

interface Col { id: string; label: string; w: number; num?: boolean; sort?: SortKey; group: string }

const LEAD: Col[] = [
  { id: "on", label: "", w: 34, group: "Account" },
  { id: "auto", label: "Auto", w: 40, group: "Account" },
  { id: "name", label: "Name", w: 132, sort: "alias", group: "Account" },
  { id: "broker", label: "Broker", w: 96, sort: "broker", group: "Account" },
  { id: "status", label: "Status", w: 118, group: "Account" },
];
/** Everything in one table: who, status, the margins, then each sign-in column. The row's buttons stay pinned at the right edge. */
const COLS: Col[] = [
  ...LEAD,
  { id: "available", label: "Available", w: 108, num: true, sort: "available", group: "Margin" },
  { id: "used", label: "Used", w: 98, num: true, group: "Margin" },
  { id: "total", label: "Total", w: 108, num: true, group: "Margin" },
  { id: "cash", label: "Cash", w: 104, num: true, group: "Margin" },
  { id: "collateral", label: "Collateral", w: 116, num: true, group: "Margin" },
  ...CRED_COLS.map((c) => ({ id: "cred:" + c.id, label: c.label, w: c.w, group: "Sign-in details" })),
  { id: "acts", label: "", w: 104, group: "" },
];

/** Neighbouring columns of one group, for the labelled row above the headings. */
const GROUPS = COLS.reduce<{ name: string; span: number }[]>((out, c) => {
  const last = out[out.length - 1];
  if (last && last.name === c.group) last.span += 1;
  else out.push({ name: c.group, span: 1 });
  return out;
}, []);

// ------------------------------------------------------- widths and filters

const loadWidths = (): Record<string, number> => {
  try {
    return JSON.parse(localStorage.getItem("colWidths") ?? "{}");
  } catch {
    return {};
  }
};
/** Column widths the user dragged, by column id. Remembered on this computer. */
const colW = signal<Record<string, number>>(loadWidths());
const widthOf = (c: Col) => colW.value[c.id] ?? c.w;
/** The tick-box columns keep their size: the pinned Name column sits just after them. */
const resizable = (c: Col) => c.id !== "on" && c.id !== "auto" && c.id !== "acts";

function saveWidths() {
  try {
    localStorage.setItem("colWidths", JSON.stringify(colW.value));
  } catch {
    /* not remembered, still applied */
  }
}

/** The grip on a heading's right edge: drag to resize, double-click to go back to the default. */
function Resizer({ col }: { col: Col }) {
  const start = (e: PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const el = e.currentTarget as HTMLElement;
    const x0 = e.clientX;
    const w0 = widthOf(col);
    el.setPointerCapture(e.pointerId);
    document.body.classList.add("resizing");
    const move = (ev: PointerEvent) => {
      const w = Math.max(56, Math.min(480, Math.round(w0 + ev.clientX - x0)));
      colW.value = { ...colW.value, [col.id]: w };
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      document.body.classList.remove("resizing");
      saveWidths();
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };
  const reset = () => {
    const { [col.id]: _gone, ...rest } = colW.value;
    colW.value = rest;
    saveWidths();
  };
  return <span class="rz" title="Drag to resize. Double-click to reset." onPointerDown={start} onDblClick={reset} onClick={(e) => e.stopPropagation()} />;
}

/** The filter typed or picked under each heading, by column id. */
const colFilters = signal<Record<string, string>>({});
const showFilters = signal(false);
const setColFilter = (id: string, v: string) => (colFilters.value = { ...colFilters.value, [id]: v });
const activeFilterCount = () => Object.values(colFilters.value).filter((v) => v.trim()).length;

/** What the column filters look at, read out of an account. */
function filterRow(a: Account): FilterRow {
  const cred: FilterRow["cred"] = {};
  for (const c of CRED_COLS) {
    const f = fieldFor(a.broker, c.id);
    cred[c.id] = { used: !!f, saved: !!f && (f.secret ? !!a.saved[f.key] : !!currentVal(a, f.key)), text: f && !f.secret ? currentVal(a, f.key) : "" };
  }
  const status: FilterRow["status"] = a.broker === "paper" ? "sim" : a.status === "in" ? "in" : a.signInError ? "failed" : "need";
  return {
    on: a.on, auto: a.autoLogin, alias: a.alias, broker: a.broker, status,
    available: a.funds?.available ?? null, used: a.funds?.used ?? null, total: a.funds?.total ?? null,
    cash: a.funds?.cash ?? null, collateral: a.funds?.collateral ?? null, cred,
  };
}

function FilterControl({ col }: { col: Col }) {
  const v = colFilters.value[col.id] ?? "";
  const set = (e: { currentTarget: HTMLInputElement | HTMLSelectElement }) => setColFilter(col.id, e.currentTarget.value);
  const pick = (opts: [string, string][]) => (
    <select class={"fin" + (v ? " on" : "")} value={v} aria-label={`Filter ${col.label || col.id}`} onChange={set}>
      <option value="">All</option>
      {opts.map(([val, label]) => <option key={val} value={val}>{label}</option>)}
    </select>
  );
  const text = (placeholder: string) => (
    <input class={"fin" + (v ? " on" : "")} value={v} placeholder={placeholder} aria-label={`Filter ${col.label}`} spellcheck={false} autocomplete="off" onInput={set} />
  );
  let body: preact.JSX.Element | null = null;
  if (col.id === "on" || col.id === "auto") {
    // Too narrow for a list: a small button that steps All, On, Off.
    const next = v === "" ? "on" : v === "on" ? "off" : "";
    body = (
      <button class={"fin cyc" + (v ? " on" : "")} title={`${col.id === "on" ? "Active" : "Auto"}: ${v || "all"}. Click to change.`} aria-label={`Filter ${col.id === "on" ? "active" : "auto"}: ${v || "all"}`} onClick={() => setColFilter(col.id, next)}>
        {v === "" ? "All" : v === "on" ? "On" : "Off"}
      </button>
    );
  }
  else if (col.id === "name") body = text("Filter");
  else if (col.id === "broker") body = pick((Object.keys(BROKERS) as BrokerId[]).map((b) => [b, BROKERS[b].name]));
  else if (col.id === "status") body = pick([["in", "Signed in"], ["need", "Not signed in"], ["failed", "Sign-in failed"], ["sim", "Simulator"]]);
  else if (col.num) body = text(">1000");
  else if (col.id.startsWith("cred:")) {
    body = SECRET_COLS.has(col.id.slice(5)) ? pick([["saved", "Saved"], ["missing", "Missing"], ["na", "Not used"]]) : text("Filter");
  }
  return body;
}

function FilterCell({ col }: { col: Col }) {
  return <th class="fcell-th"><FilterControl col={col} /></th>;
}

/** Columns with a filter. The two tick-box columns use the filter row only; the button column has none. */
const filterable = (c: Col) => c.id !== "acts" && c.id !== "on" && c.id !== "auto";

/** The filter open beside a heading, placed under its icon. */
const openFilter = signal<{ id: string; x: number; y: number } | null>(null);

/** The funnel beside a heading. Quiet until hovered; sky blue while that column is filtered. */
function FilterIcon({ col }: { col: Col }) {
  if (!filterable(col)) return null;
  const active = !!(colFilters.value[col.id] ?? "").trim();
  const open = openFilter.value?.id === col.id;
  return (
    <button
      class={"fbtn" + (active ? " on" : "") + (open ? " open" : "")}
      title={active ? `Filtered: ${colFilters.value[col.id]}` : `Filter ${col.label.toLowerCase()}`}
      aria-label={`Filter ${col.label}`} aria-expanded={open}
      onClick={(e) => {
        e.stopPropagation();
        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
        openFilter.value = open ? null : { id: col.id, x: Math.round(r.right), y: Math.round(r.bottom + 6) };
      }}
    >
      <I.Funnel size={12} />
    </button>
  );
}

/** The pop-up with one column's filter. Closes on Escape, a click elsewhere, or a scroll. */
function FilterPop() {
  const o = openFilter.value;
  const col = o ? COLS.find((c) => c.id === o.id) : undefined;
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!o) return;
    box.current?.querySelector<HTMLElement>("input, select, button.fin")?.focus();
    const close = () => (openFilter.value = null);
    const onDown = (e: MouseEvent) => {
      const t = e.target as Element;
      if (!t.closest(".fpop") && !t.closest(".fbtn")) close();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", close);
    document.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", close);
      document.removeEventListener("scroll", close, true);
    };
  }, [o?.id]);
  if (!o || !col) return null;
  const width = 232;
  const left = Math.max(8, Math.min(o.x - width, window.innerWidth - width - 8));
  const v = colFilters.value[col.id] ?? "";
  const hint = col.num ? "A number, or >, >=, <, <= and a number." : col.id.startsWith("cred:") && SECRET_COLS.has(col.id.slice(5)) ? "Saved, missing, or not used by the broker." : "";
  return (
    <div ref={box} class="fpop" style={{ left: `${left}px`, top: `${o.y}px`, width: `${width}px` }} role="dialog" aria-label={`Filter ${col.label}`}>
      <div class="fpop-head">
        <I.Funnel size={12} />
        <span>Filter {col.label.toLowerCase()}</span>
      </div>
      <FilterControl col={col} />
      {hint && <p class="fpop-hint">{hint}</p>}
      <div class="fpop-foot">
        <button class="linkbtn" disabled={!v} onClick={() => setColFilter(col.id, "")}>Clear</button>
        <button class="btn primary small" onClick={() => (openFilter.value = null)}>Done</button>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------- cells

function StatusCell({ a }: { a: Account }) {
  if (a.broker === "paper") return <span class="state"><i class="dot" />Simulator</span>;
  if (a.unsaved) return <span class="state"><i class="dot mid" />New</span>;
  if (a.status === "in") return <span class="state ok"><i class="dot ok" />Signed in</span>;
  if (a.signInError) return <span class="state linklike"><i class="dot bad" />Sign-in failed</span>;
  return <span class="state"><i class="dot" />Not signed in</span>;
}

/** One margin figure, with the broker's own breakdown on hover. */
function FundsCell({ a, pick }: { a: Account; pick: (f: NonNullable<Account["funds"]>) => number }) {
  const f = a.funds;
  if (!f) return <span class="money zero" title={a.fundsError || undefined}>–</span>;
  const fmt = (n: number) => n.toLocaleString("en-IN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const lines = [
    `Available: ${fmt(f.available)}`,
    ...(f.cash !== null ? [`  Cash: ${fmt(f.cash)}`] : []),
    ...(f.collateral ? [`  Collateral: ${fmt(f.collateral)}`] : []),
    `Used: ${fmt(f.used)}`,
    ...f.details.map(([label, n]) => `  ${label}: ${fmt(n)}`),
  ];
  const tip = [a.fundsError ? `Could not refresh: ${a.fundsError}` : "", ...lines].filter(Boolean).join("\n");
  return (
    <span class={"fcell" + (a.fundsError ? " stale" : "")} title={tip || undefined}>
      <Money n={pick(f)} />
    </span>
  );
}

/** Enter saves the row, Escape throws away what was typed in it. */
function rowKeys(a: Account) {
  return (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void saveRow(a);
    } else if (e.key === "Escape") {
      e.preventDefault();
      discardRow(a);
    }
  };
}

/** One sign-in cell. A secret that is already saved shows "saved" and is never read back; typing replaces it. */
function CredCell({ a, col }: { a: Account; col: (typeof CRED_COLS)[number]["id"] }) {
  const broker = brokerOf(a);
  const f = fieldFor(broker, col);
  if (!f) return <td class="na">–</td>;
  const d = drafts.value[a.id];
  const savedHere = f.secret && !!a.saved[f.key] && broker === a.broker;
  const shownSaved = savedHere && revealTyped.value ? revealed.value[a.id]?.[f.key] ?? "" : "";
  const value = d?.vals[f.key] ?? (f.secret ? shownSaved : currentVal(a, f.key));
  const common = {
    class: "cellin" + (savedHere && !value ? " saved" : "") + (value ? " has-copy" : ""),
    name: `${a.id}-${f.key}`,
    value,
    placeholder: savedHere ? "••••••••" : f.label,
    title: savedHere && !value ? `${f.label} is saved. Use the eye above to see it, or type to replace it.` : `${f.label}. ${f.where}`,
    "aria-label": `${a.alias || "New account"}: ${f.label}${savedHere ? " (saved)" : ""}`,
    "aria-invalid": d?.invalid.includes(f.key) ? ("true" as const) : undefined,
    autocomplete: "off",
    spellcheck: false,
    onInput: (e: { currentTarget: HTMLInputElement }) => patchDraft(a.id, { val: [f.key, e.currentTarget.value] }),
    onKeyDown: rowKeys(a),
  };
  const hide = f.secret && !revealTyped.value;
  return (
    <td class="cellc">
      {hide ? <input type="password" {...common} /> : <input type="text" {...common} />}
      {value && (!f.secret || revealTyped.value) && (
        <button class="copy" tabIndex={-1} title={`Copy ${f.label}`} aria-label={`Copy ${f.label}`} onClick={() => copyValue(f.label, value)}>
          <I.Copy />
        </button>
      )}
    </td>
  );
}

function Cell({ a, col }: { a: Account; col: Col }) {
  const busy = signingIn.value.has(a.id);
  const dirty = isDirty(a);
  switch (col.id) {
    case "on":
      return <td><input type="checkbox" checked={a.on} aria-label={`${a.alias || "New account"} active`} onChange={(e) => patchRow(a, { on: e.currentTarget.checked })} /></td>;
    case "auto":
      return (
        <td>
          {brokerOf(a) !== "paper" && (
            <input type="checkbox" checked={a.autoLogin} aria-label={`${a.alias || "New account"}: include in Login all`} onChange={(e) => patchRow(a, { autoLogin: e.currentTarget.checked })} />
          )}
        </td>
      );
    case "name": {
      const d = drafts.value[a.id];
      return (
        <td class="cellc">
          <input
            class="cellin name" name={`${a.id}-alias`} value={d?.alias ?? a.alias} placeholder="Name" autocomplete="off"
            aria-label="Account name" aria-invalid={d?.invalid.includes("alias") ? "true" : undefined}
            onInput={(e) => patchDraft(a.id, { alias: e.currentTarget.value })} onKeyDown={rowKeys(a)}
          />
        </td>
      );
    }
    case "broker":
      return a.unsaved ? (
        <td class="cellc">
          <select class="cellin" aria-label="Broker" value={brokerOf(a)} onChange={(e) => patchDraft(a.id, { broker: e.currentTarget.value as BrokerId, invalid: [] })}>
            {(Object.keys(BROKERS) as BrokerId[]).map((b) => <option key={b} value={b}>{BROKERS[b].name}</option>)}
          </select>
        </td>
      ) : (
        <td>{BROKERS[a.broker].name}</td>
      );
    case "status":
      return <td><StatusCell a={a} /></td>;
    case "available":
      return <td class="num"><FundsCell a={a} pick={(f) => f.available} /></td>;
    case "used":
      return <td class="num"><FundsCell a={a} pick={(f) => f.used} /></td>;
    case "total":
      return <td class="num"><FundsCell a={a} pick={(f) => f.total} /></td>;
    case "cash":
      return <td class="num">{a.funds && a.funds.cash === null ? <span class="money zero" title="This broker does not report cash on its own">–</span> : <FundsCell a={a} pick={(f) => f.cash ?? 0} />}</td>;
    case "collateral":
      return <td class="num"><FundsCell a={a} pick={(f) => f.collateral} /></td>;
    case "acts":
      return (
        <td class="acts">
          {dirty ? (
            <>
              <button class="ic" title="Save and sign in (Enter)" aria-label={`Save ${a.alias || "new account"}`} disabled={busy} onClick={() => saveRow(a)}>
                <span class={busy ? "spin" : ""}>{busy ? <I.Refresh /> : <I.Check />}</span>
              </button>
              <button class="ic" title="Discard (Esc)" aria-label="Discard changes" disabled={busy} onClick={() => discardRow(a)}><I.X /></button>
            </>
          ) : (
            <>
              {a.broker !== "paper" && (
                <button class="ic" title={a.status === "in" ? "Sign in again" : "Sign in"} aria-label={`Sign in ${a.alias}`} disabled={busy} onClick={() => doSignIn(a)}>
                  <span class={busy ? "spin" : ""}><I.Refresh /></span>
                </button>
              )}
              <button class="ic danger" title="Delete account" aria-label={`Delete ${a.alias}`} onClick={() => askDelete(a)}><I.Trash /></button>
            </>
          )}
        </td>
      );
    default:
      return col.id.startsWith("cred:") ? <CredCell a={a} col={col.id.slice(5) as never} /> : <td />;
  }
}

function Row({ a, i, cols }: { a: Account; i: number; cols: Col[] }) {
  const dirty = isDirty(a);
  const err = a.signInError && a.status !== "in" && !a.unsaved ? a.signInError : "";
  // Signed in, but the broker would not give the margin: say why, quietly, under the row.
  const fundsWarn = !err && a.status === "in" && a.fundsError ? a.fundsError : "";
  return (
    <>
      <tr class={"row" + (a.unsaved ? " new" : "") + (dirty ? " dirty" : "")} style={{ "--i": Math.min(i, 14) } as never}>
        {cols.map((c) => <Cell key={c.id} a={a} col={c} />)}
      </tr>
      {err && (
        <tr class="errrow"><td colSpan={cols.length}>{err}</td></tr>
      )}
      {fundsWarn && (
        <tr class="errrow warnrow"><td colSpan={cols.length}>Margin: {fundsWarn.replace(/^(broker rejected \(login\)|network error, nothing was sent): /, "")}</td></tr>
      )}
    </>
  );
}

// -------------------------------------------------------------------- hints

/** While a row is being filled in, says what that broker needs and where to find the next thing. */
function HintBar({ a }: { a: Account }) {
  const broker = brokerOf(a);
  const def = BROKERS[broker];
  const d = drafts.value[a.id];
  if (def.fields.length === 0) {
    return <div class="hintbar"><b>{def.name}</b><span class="next">{def.note}</span></div>;
  }
  const done = (key: string) => {
    const f = def.fields.find((x) => x.key === key)!;
    if (f.secret) return !!(d?.vals[key] ?? "").trim() || (!!a.saved[key] && broker === a.broker);
    return !!(d?.vals[key] !== undefined ? d.vals[key].trim() : currentVal(a, key).trim());
  };
  const next = def.fields.find((f) => !done(f.key));
  return (
    <div class="hintbar" role="status">
      <b>{def.name} needs</b>
      {def.fields.map((f) => (
        <span key={f.key} class={"need" + (done(f.key) ? " done" : "")} title={f.where}>
          {done(f.key) && <I.Check />}{f.label}
        </span>
      ))}
      <span class="next">
        {next ? <><strong>{next.label}.</strong> {next.where}</> : "All filled in. Press Enter, or the tick at the end of the row, to save and sign in."}
      </span>
      {broker === a.broker && def.note && next && <span class="next">{def.note}</span>}
    </div>
  );
}

// ------------------------------------------------------------------- screen

function SortTh({ col }: { col: Col }) {
  const s = sortState.value;
  const k = col.sort!;
  const on = s.key === k;
  return (
    <th class={col.num ? "num" : ""} aria-sort={on ? (s.dir === "asc" ? "ascending" : "descending") : "none"}>
      <button class="thbtn" onClick={() => (sortState.value = nextSort(s, k))} title={`Sort by ${col.label.toLowerCase()}`}>
        {col.label}
        <span class={"sorticon" + (on ? " on" : "")}>{on ? (s.dir === "asc" ? <I.ChevUp /> : <I.ChevDown />) : <I.ChevSort />}</span>
      </button>
      <FilterIcon col={col} />
      {resizable(col) && <Resizer col={col} />}
    </th>
  );
}

function Empty() {
  const down = engine.value.state === "down" && !accountsLoaded.value;
  return (
    <div class="canvas">
      <div class="empty">
        <span class="glyph"><I.NavAccounts /></span>
        <h2>{down ? "The engine is not running" : "No accounts yet"}</h2>
        <p>
          {down
            ? "Start it with start.bat in the project folder, then this list fills in on its own."
            : "Add a broker account. It appears as a row where you fill in its sign-in details, with hints for the broker you pick."}
        </p>
        {!down && <button class="btn primary" onClick={addAccount}><I.Plus /> Add account</button>}
      </div>
    </div>
  );
}

function Skeleton() {
  return (
    <div class="canvas">
      <div class="skel" aria-hidden="true">
        {[0, 1, 2, 3].map((i) => <div key={i} />)}
      </div>
    </div>
  );
}

/** Marks which edges of the table area have more to scroll to, so a pinned column draws its edge only then. */
function useScrollEdges() {
  const ref = useRef<HTMLDivElement>(null);
  const update = () => {
    const el = ref.current;
    if (!el) return;
    el.dataset.left = String(el.scrollLeft > 0);
    el.dataset.right = String(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
  };
  useEffect(() => {
    update();
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  });
  return { ref, onScroll: update };
}

function AccountsScreen() {
  const edges = useScrollEdges();
  const list = accounts.value;
  const adding = newAccount.value;
  const f = filters.value;
  const setFilter = (p: Partial<typeof f>) => (filters.value = { ...filters.value, ...p });
  const cf = colFilters.value;
  const rows = viewRows(list, f, sortState.value, (id) => BROKERS[id as BrokerId]?.name ?? id).filter((a) => passes(filterRow(a), cf));
  const nFilters = activeFilterCount();
  const narrowed = filtersActive(f) || nFilters > 0;
  const active = list.filter((a) => a.on).length;
  const allOn = rows.length > 0 && rows.every((a) => a.on);
  const someOn = rows.some((a) => a.on);
  const marked = list.filter((a) => a.autoLogin && a.broker !== "paper").length;
  const loading = !accountsLoaded.value && engine.value.state !== "down";
  const cols = COLS;
  const width = cols.reduce((t, c) => t + widthOf(c), 0);
  // The row being filled in: the new one, else the first saved row with unsaved edits.
  const editing = adding ?? list.find((a) => isDirty(a)) ?? null;
  const empty = list.length === 0 && !adding;
  return (
    <section class="screen">
      <div class="strip">
        <h1>Accounts</h1>
        <span class="count">
          {list.length === 0 ? "None yet" : narrowed ? `${rows.length} of ${list.length} shown` : `${active} of ${list.length} active`}
        </span>
        {list.length > 0 && (
          <>
            <span class="rule" />
            <label class="search">
              <I.Search />
              <input type="search" placeholder="Search accounts" aria-label="Search accounts" value={f.search} onInput={(e) => setFilter({ search: e.currentTarget.value })} />
            </label>
            <button class={"btn" + (showFilters.value ? " on" : "")} aria-pressed={showFilters.value} onClick={() => (showFilters.value = !showFilters.value)} title="A filter under every heading">
              <I.Funnel /> Filters{nFilters > 0 && <span class="count-badge">{nFilters}</span>}
            </button>
            {narrowed && (
              <button class="btn quiet" onClick={() => { filters.value = noFilters(); colFilters.value = {}; }}>Clear</button>
            )}
          </>
        )}
        <span class="grow" />
        <button class="btn" disabled={!!adding} onClick={addAccount}><I.Plus /> Add account</button>
        <button class="btn" disabled={busyFunds.value || !list.some((a) => a.status === "in")} onClick={doRefreshFunds} title="Read every signed-in account's margin from its broker again">
          <span class={busyFunds.value ? "spin" : ""}><I.Refresh /></span>
          {busyFunds.value ? "Reading…" : "Refresh margins"}
        </button>
        <button class="btn primary" disabled={busyAll.value || marked === 0} onClick={doLoginAll} title="Sign in every account with Auto ticked, all at once">
          <span class={busyAll.value ? "spin" : ""}><I.Refresh /></span>
          {busyAll.value ? "Signing in…" : `Login all${marked ? ` (${marked})` : ""}`}
        </button>
      </div>

      {list.length > 0 && <Figures list={list} />}
      {editing && <HintBar a={editing} />}

      {loading ? (
        <Skeleton />
      ) : empty ? (
        <Empty />
      ) : (
        <div class="canvas" ref={edges.ref} onScroll={edges.onScroll}>
          <table class="grid-table" style={{ width: `${width}px` }}>
            <colgroup>{cols.map((c) => <col key={c.id} style={{ width: `${widthOf(c)}px` }} />)}</colgroup>
            <thead>
              <tr class="grp">
                {GROUPS.map((g, i) => (
                  <th key={i} colSpan={g.span} class="grp">
                    {g.name}
                    {g.name === "Sign-in details" && (
                      <>
                        <button
                          class="ic mini" aria-pressed={revealTyped.value}
                          title={revealTyped.value ? "Hide the values" : `Show the saved values for ${REVEAL_SECONDS} seconds`}
                          aria-label={revealTyped.value ? "Hide sign-in values" : "Show sign-in values"}
                          onClick={toggleValues}
                        >
                          {revealTyped.value ? <I.EyeOff /> : <I.Eye />}
                        </button>
                        {revealTyped.value && <span class="countdown" aria-live="off">hides in {revealLeft.value}s</span>}
                      </>
                    )}
                  </th>
                ))}
              </tr>
              <tr class="heads">
                {cols.map((c) =>
                  c.id === "on" ? (
                    <th key={c.id}>
                      <input
                        type="checkbox" aria-label="Activate or deactivate all shown accounts" title="Activate or deactivate all shown accounts"
                        checked={allOn} ref={(el) => { if (el) el.indeterminate = someOn && !allOn; }}
                        onChange={(e) => setEnabled(rows.map((a) => a.id), e.currentTarget.checked)}
                      />
                    </th>
                  ) : c.sort ? (
                    <SortTh key={c.id} col={c} />
                  ) : (
                    <th key={c.id} class={c.num ? "num" : ""} title={c.id === "auto" ? "Include in Login all" : undefined}>
                      {c.label ? <span class="thlabel">{c.label}</span> : <span class="vh">Actions</span>}
                      <FilterIcon col={c} />
                      {resizable(c) && <Resizer col={c} />}
                    </th>
                  ),
                )}
              </tr>
              {showFilters.value && (
                <tr class="filters">{cols.map((c) => <FilterCell key={c.id} col={c} />)}</tr>
              )}
            </thead>
            <tbody>
              {adding && <Row key={adding.id} a={adding} i={0} cols={cols} />}
              {rows.map((a, i) => <Row key={a.id} a={a} i={i} cols={cols} />)}
            </tbody>
          </table>
          {rows.length === 0 && !adding && (
            <p class="nomatch">No account matches. <button class="linkbtn" onClick={() => { filters.value = noFilters(); colFilters.value = {}; }}>Clear the filters</button></p>
          )}
        </div>
      )}
    </section>
  );
}

// --------------------------------------------------------------------- app

export function App() {
  useEngine();
  return (
    <div class="shell">
      <Header />
      <div class="body">
        <Nav />
        <main><AccountsScreen /></main>
      </div>
      <FilterPop />
      <ConfirmHost />
      <div class={"toast" + (toast.value ? " show" : "")} role="status" aria-live="polite">{toast.value}</div>
    </div>
  );
}

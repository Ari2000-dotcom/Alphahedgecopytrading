import { useEffect } from "preact/hooks";
import { invoke } from "@tauri-apps/api/core";
import {
  accounts, addAccount, BROKERS, engine, engineUrl, filters, openId, removeAccount, setEnabled, setTheme, showToast,
  sortState, theme, toast, updateAccount, wide, type Account, type BrokerId, type Theme,
} from "./store";
import { filtersActive, nextSort, noFilters, viewRows, type SortKey } from "./grid";
import { askConfirm, ConfirmHost } from "./confirm";
import { Drawer } from "./drawer";
import { moneyParts } from "./format";
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

function useEnginePing() {
  useEffect(() => {
    ping();
    const t = window.setInterval(ping, 5000);
    document.addEventListener("visibilitychange", ping);
    return () => {
      clearInterval(t);
      document.removeEventListener("visibilitychange", ping);
    };
  }, []);
}

// ------------------------------------------------------------------ shell

function Header() {
  const e = engine.value;
  const mode = e.state === "up" ? (e.broker === "paper" ? "PAPER" : "LIVE") : "OFFLINE";
  const cls = mode === "LIVE" ? "badge live" : mode === "PAPER" ? "badge paper" : "badge off";
  return (
    <header class="top">
      <strong>AlphaHedge Desk</strong>
      <span class={cls}>{mode}</span>
      <span class="chip">Sample data</span>
      <span class="grow" />
      <ThemeSwitch />
      <span class="status">
        <i class={"dot " + (e.state === "up" ? "ok" : e.state === "down" ? "bad" : "mid")} />
        {e.state === "up" ? `Engine connected · ${e.ms} ms` : e.state === "down" ? "Engine not running" : "Checking engine…"}
      </span>
    </header>
  );
}

const NAV: { label: string; icon: () => preact.JSX.Element; live?: boolean }[] = [
  { label: "Trade", icon: I.NavBolt },
  { label: "Orders", icon: I.NavList },
  { label: "Positions", icon: I.NavChart },
  { label: "Accounts", icon: I.NavUsers, live: true },
  { label: "Risk", icon: I.NavShield },
  { label: "Alerts", icon: I.NavBell },
];

function Nav() {
  return (
    <nav class="nav" aria-label="Sections">
      {NAV.map((n) => (
        <button
          key={n.label}
          class={"navbtn" + (n.live ? " on" : "")}
          title={n.live ? n.label : `${n.label} (not built yet)`}
          aria-label={n.label}
          aria-current={n.live ? "page" : undefined}
          onClick={() => !n.live && showToast(`${n.label} is not built yet`)}
        >
          <n.icon />
        </button>
      ))}
    </nav>
  );
}

// ------------------------------------------------------------------- numbers

/** Money the way a broker shows it: grouped, two decimals, dimmed decimals,
 *  and (for profit and loss) green, red or gray. */
function Money({ n, plus = false, color = false }: { n: number; plus?: boolean; color?: boolean }) {
  const p = moneyParts(n, plus);
  if (!p) return <span class="money zero">–</span>;
  return (
    <span class={"money" + (color ? " " + p.tone : "")}>
      {p.sign}{p.int}<span class="frac">{p.frac}</span>
    </span>
  );
}

function Stats({ list }: { list: Account[] }) {
  const on = list.filter((a) => a.on);
  const avail = on.reduce((s, a) => s + a.available, 0);
  const mtm = on.reduce((s, a) => s + a.mtm, 0);
  const signed = on.filter((a) => a.status === "in").length;
  return (
    <div class="stats">
      <div><span class="lab">Available margin</span><b class="big"><Money n={avail} /></b></div>
      <div><span class="lab">MTM today</span><b class="big"><Money n={mtm} plus color /></b></div>
      <div><span class="lab">Signed in</span><b class="big">{signed}<span class="of"> of {on.length}</span></b></div>
    </div>
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

// ---------------------------------------------------------------- accounts

function StatusIcon({ s }: { s: Account["status"] }) {
  if (s === "in") return <span class="st ok" title="Signed in"><I.CheckCircle /></span>;
  if (s === "need") return <span class="st warn" title="Needs sign-in"><I.Alert /></span>;
  if (s === "sim") return <span class="st" title="Simulator"><I.Flask /></span>;
  return <span class="st dim" title="Not signed in"><I.Dashed /></span>;
}

function AliasCell({ a }: { a: Account }) {
  return (
    <input
      class="cell"
      key={a.alias}
      defaultValue={a.alias}
      aria-label="Alias"
      onKeyDown={(e) => {
        const el = e.currentTarget;
        if (e.key === "Enter") {
          const v = el.value.trim();
          if (!v) { showToast("Enter a name for the account"); return; }
          updateAccount(a.id, { alias: v });
          el.blur();
          showToast("Row saved");
        } else if (e.key === "Escape") {
          el.value = a.alias;
          el.blur();
          showToast("Changes cancelled");
        }
      }}
    />
  );
}

function askDelete(a: Account) {
  const body = [
    `${BROKERS[a.broker].name}${a.userId ? " · " + a.userId : ""}. This removes its saved sign-in details and settings from this app.`,
    "Positions and orders at the broker are not touched.",
  ];
  if (a.status === "in") {
    body.push("It is signed in right now. Close its positions first if you still need its stop-losses to work.");
  }
  askConfirm({
    title: `Delete ${a.alias}?`,
    body,
    confirmLabel: "Delete account",
    danger: true,
    onConfirm: () => {
      removeAccount(a.id);
      showToast(`${a.alias} deleted`);
    },
  });
}

function Row({ a }: { a: Account }) {
  const sel = openId.value === a.id;
  return (
    <>
      <tr class={"row" + (sel ? " sel" : "")}>
        <td><input type="checkbox" checked={a.on} aria-label="Enabled" onChange={(e) => updateAccount(a.id, { on: e.currentTarget.checked })} /></td>
        <td><StatusIcon s={a.status} /></td>
        <td><AliasCell a={a} /></td>
        <td>{BROKERS[a.broker].name}</td>
        <td class="sub">{a.userId || "–"}</td>
        <td class="num">{a.available ? <Money n={a.available} /> : <span class="money zero">–</span>}</td>
        <td class="num"><Money n={a.mtm} plus color /></td>
        {wide.value && <td class="num"><Money n={a.mtmMis} plus color /></td>}
        {wide.value && <td class="num"><Money n={a.mtmNrml} plus color /></td>}
        <td>
          <button class={"pill " + (a.marketOrders ? "ok" : "warn")} onClick={() => updateAccount(a.id, { marketOrders: !a.marketOrders })} title="Click to change">
            {a.marketOrders ? "Allowed" : "Limit only"}
          </button>
        </td>
        <td class="acts">
          {a.status === "in" ? (
            <button class="ic" title="Sign out" aria-label="Sign out" onClick={() => showToast("Sign-out arrives with the engine's accounts store")}><I.Logout /></button>
          ) : (
            <button class="ic" title="Sign in" aria-label="Sign in" onClick={() => showToast("Sign-in arrives with the engine's accounts store")}><I.Refresh /></button>
          )}
          <button class={"ic" + (sel ? " on" : "")} title="Credentials and settings" aria-label="Credentials and settings" onClick={() => (openId.value = sel ? null : a.id)}><I.Key /></button>
          <button class="ic danger" title="Square off this account" aria-label="Square off this account" onClick={() => showToast("Square-off arrives with the kill switch")}><I.Stop /></button>
          <button class="ic" title="Delete account" aria-label="Delete account" onClick={() => askDelete(a)}><I.Trash /></button>
        </td>
      </tr>
      {sel && (
        <tr class="drawer-row">
          <td colSpan={wide.value ? 11 : 9}><Drawer a={a} /></td>
        </tr>
      )}
    </>
  );
}

function SortTh({ k, label, num = false }: { k: SortKey; label: string; num?: boolean }) {
  const s = sortState.value;
  const on = s.key === k;
  return (
    <th class={(num ? "num " : "") + "col-" + k} aria-sort={on ? (s.dir === "asc" ? "ascending" : "descending") : "none"}>
      <button class="thbtn" onClick={() => (sortState.value = nextSort(s, k))}>
        {label}
        {on && (s.dir === "asc" ? <I.ChevUp /> : <I.ChevDown />)}
      </button>
    </th>
  );
}

function Toolbar({ shown, total }: { shown: number; total: number }) {
  const f = filters.value;
  const set = (p: Partial<typeof f>) => (filters.value = { ...filters.value, ...p });
  return (
    <div class="toolbar">
      <label class="search">
        <I.Search />
        <input type="search" placeholder="Filter by name, user ID or broker" aria-label="Filter accounts" value={f.search} onInput={(e) => set({ search: e.currentTarget.value })} />
      </label>
      <select aria-label="Broker" value={f.broker} onChange={(e) => set({ broker: e.currentTarget.value })}>
        <option value="all">All brokers</option>
        {(Object.keys(BROKERS) as BrokerId[]).map((b) => <option key={b} value={b}>{BROKERS[b].name}</option>)}
      </select>
      <select aria-label="Status" value={f.status} onChange={(e) => set({ status: e.currentTarget.value })}>
        <option value="all">Any status</option>
        <option value="in">Signed in</option>
        <option value="need">Needs sign-in</option>
        <option value="off">Not signed in</option>
        <option value="sim">Simulator</option>
      </select>
      {filtersActive(f) && <button class="btn" onClick={() => (filters.value = noFilters())}>Clear</button>}
      <span class="grow" />
      {filtersActive(f) && <span class="sub">Showing {shown} of {total}</span>}
    </div>
  );
}

function AccountsScreen() {
  const list = accounts.value;
  const view = viewRows(list, filters.value, sortState.value, (id) => BROKERS[id as BrokerId]?.name ?? id);
  const on = list.filter((a) => a.on).length;
  const allOn = view.length > 0 && view.every((a) => a.on);
  const someOn = view.some((a) => a.on);
  return (
    <section class="screen">
      <div class="bar">
        <h1>Accounts</h1>
        <span class="sub">{on} of {list.length} enabled</span>
        <span class="grow" />
        <button class="btn" onClick={addAccount}><I.Plus /> Add account</button>
      </div>
      <Stats list={list} />
      <Toolbar shown={view.length} total={list.length} />
      <table class="grid-table">
        <colgroup>
          <col style="width:40px" /><col style="width:36px" /><col style="width:13%" /><col style="width:11%" /><col style="width:9%" />
          <col /><col />{wide.value && <col />}{wide.value && <col />}<col style="width:104px" /><col style="width:150px" />
        </colgroup>
        <thead>
          <tr>
            <th>
              <input
                type="checkbox" aria-label="Enable or disable all shown accounts" title="Enable or disable all shown accounts"
                checked={allOn} ref={(el) => { if (el) el.indeterminate = someOn && !allOn; }}
                onChange={(e) => setEnabled(view.map((a) => a.id), e.currentTarget.checked)}
              />
            </th>
            <th></th>
            <SortTh k="alias" label="Alias" /><SortTh k="broker" label="Broker" /><SortTh k="userId" label="User ID" />
            <SortTh k="available" label="Available" num /><SortTh k="mtm" label="MTM (all)" num />
            {wide.value && <SortTh k="mtmMis" label="MIS MTM" num />}{wide.value && <SortTh k="mtmNrml" label="NRML MTM" num />}
            <th>Market</th><th class="num">Actions</th>
          </tr>
        </thead>
        <tbody>
          {view.map((a) => <Row key={a.id} a={a} />)}
          <tr class="addrow"><td colSpan={wide.value ? 11 : 9}><button class="linkbtn" onClick={addAccount}>+ Click here to add a new account</button></td></tr>
        </tbody>
      </table>
      {list.length === 0 && <p class="empty">No accounts yet. Add your first broker account.</p>}
      {list.length > 0 && view.length === 0 && (
        <p class="empty">No account matches the filter. <button class="linkbtn" onClick={() => (filters.value = noFilters())}>Clear it</button></p>
      )}
      <footer class="hints">
        <span>Turning an account on or off applies from the next start of trading</span>
        <span class="grow" />
        <span>Enter saves the row</span><span>Esc cancels</span><span>Tab moves to the next field</span>
      </footer>
    </section>
  );
}

// --------------------------------------------------------------------- app

export function App() {
  useEnginePing();
  return (
    <div class="shell">
      <Header />
      <div class="body">
        <Nav />
        <main><AccountsScreen /></main>
      </div>
      <ConfirmHost />
      <div class={"toast" + (toast.value ? " show" : "")} role="status" aria-live="polite">{toast.value}</div>
    </div>
  );
}

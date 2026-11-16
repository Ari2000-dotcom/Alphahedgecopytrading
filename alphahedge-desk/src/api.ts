// Talks to the engine's accounts routes, through the app's host process (which
// holds the engine token). Secrets only ever go out from here; the engine never
// sends one back, only which ones are saved.

import { invoke } from "./host";
import { accounts, engineUrl, noRisk, type Account, type BrokerId, type Funds, type Status } from "./store";

/** One account as the engine reports it. */
export interface EngineAccount {
  id: string;
  alias: string;
  broker: BrokerId;
  user_id: string;
  auto_login: boolean;
  fields: Record<string, string>;
  settings: Partial<Settings> | null;
  saved: Record<string, boolean>;
  session: { status: Status; error: string | null; logged_in_at: number | null; available: number | null; funds: Funds | null; funds_error: string | null };
}

/** What the app keeps per account besides the sign-in details; the engine stores it as is. */
type Settings = Pick<
  Account,
  "on" | "marketOrders" | "slPct" | "slFailure" | "outbound" | "qtyMultiplier" | "exposure" | "risk" | "alerts" | "historicalApi"
>;

function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  return invoke<T>("accounts_call", { url: engineUrl.value, method, path, body: body ?? null });
}

/** Turns what the engine stores into a row for the grid. */
export function fromEngine(e: EngineAccount, prev?: Account): Account {
  const s = e.settings ?? {};
  return {
    id: e.id, alias: e.alias, broker: e.broker, userId: e.user_id, autoLogin: e.auto_login,
    fields: e.fields, saved: e.saved,
    on: s.on ?? false,
    marketOrders: s.marketOrders ?? false,
    slPct: s.slPct ?? 50,
    slFailure: s.slFailure ?? "exit",
    outbound: s.outbound ?? "VPS IP 1",
    qtyMultiplier: s.qtyMultiplier ?? 1,
    exposure: s.exposure ?? 0,
    risk: { ...noRisk(), ...(s.risk ?? {}) },
    alerts: { telegramIds: "", email: "", authRequired: false, ...(s.alerts ?? {}) },
    historicalApi: s.historicalApi ?? false,
    status: e.session.status,
    signInError: e.session.error ?? "",
    available: e.session.available ?? 0,
    funds: e.session.funds,
    fundsError: e.session.funds_error ?? "",
    // Not read from the broker yet.
    mtm: prev?.mtm ?? 0, mtmMis: prev?.mtmMis ?? 0, mtmNrml: prev?.mtmNrml ?? 0,
  };
}

function body(a: Account, secrets: Record<string, string>) {
  const settings: Settings = {
    on: a.on, marketOrders: a.marketOrders, slPct: a.slPct, slFailure: a.slFailure, outbound: a.outbound,
    qtyMultiplier: a.qtyMultiplier, exposure: a.exposure, risk: a.risk, alerts: a.alerts, historicalApi: a.historicalApi,
  };
  return { alias: a.alias, broker: a.broker, user_id: a.userId, auto_login: a.autoLogin, fields: a.fields, secrets, settings };
}

/** Saves one account. Secrets left out keep what the engine already has. */
export async function saveAccount(a: Account, secrets: Record<string, string> = {}): Promise<Account> {
  const e = await call<EngineAccount>("PUT", `/accounts/${a.id}`, body(a, secrets));
  return fromEngine(e, a);
}

/** Signs the account in now. Throws the engine's message if the broker says no. */
export async function signIn(id: string): Promise<Account> {
  const e = await call<EngineAccount>("POST", `/accounts/${id}/login`);
  return fromEngine(e, accounts.value.find((a) => a.id === id));
}

export async function deleteAccount(id: string): Promise<void> {
  await call("DELETE", `/accounts/${id}`);
}

/** Reads every account from the engine and replaces the grid with them. */
export async function loadAccounts(): Promise<void> {
  const r = await call<{ accounts: EngineAccount[] }>("GET", "/accounts");
  const before = new Map(accounts.value.map((a) => [a.id, a]));
  const fresh = r.accounts.map((e) => fromEngine(e, before.get(e.id)));
  // Rows added in the app but not saved yet stay until they are saved or deleted.
  const unsaved = accounts.value.filter((a) => a.unsaved && !fresh.some((f) => f.id === a.id));
  accounts.value = [...fresh, ...unsaved];
}

export interface LoginAllResult {
  id: string;
  ok: boolean;
  error?: string;
  account: EngineAccount | null;
}

/** Signs in every account whose Auto box is ticked, all at once. */
export async function signInAll(): Promise<LoginAllResult[]> {
  const r = await call<{ results: LoginAllResult[] }>("POST", "/accounts/login-all");
  const before = new Map(accounts.value.map((a) => [a.id, a]));
  const byId = new Map(r.results.filter((x) => x.account).map((x) => [x.id, x.account as EngineAccount]));
  accounts.value = accounts.value.map((a) => {
    const e = byId.get(a.id);
    return e ? fromEngine(e, before.get(a.id)) : a;
  });
  return r.results;
}

/** Reads every signed-in account's margin from its broker again. */
export async function refreshFunds(): Promise<void> {
  const r = await call<{ accounts: EngineAccount[] }>("POST", "/accounts/funds");
  const before = new Map(accounts.value.map((a) => [a.id, a]));
  const byId = new Map(r.accounts.map((e) => [e.id, e]));
  accounts.value = accounts.value.map((a) => {
    const e = byId.get(a.id);
    return e ? fromEngine(e, before.get(a.id)) : a;
  });
}

/** One account's saved sign-in values, read back only because the user asked to see them. */
export async function fetchSecrets(id: string): Promise<Record<string, string>> {
  const r = await call<{ secrets: Record<string, string> }>("POST", `/accounts/${id}/secrets`);
  return r.secrets ?? {};
}

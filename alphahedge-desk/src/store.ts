import { signal } from "@preact/signals";
import { noFilters, type Filters, type Sort } from "./grid";

export type BrokerId = "zerodha" | "angel" | "kotak" | "upstox" | "nubra" | "paper";
export type Status = "in" | "need" | "off" | "sim";

export interface Field {
  key: string;
  label: string;
  /** Secret fields are write-only: once saved they are never shown again. */
  secret: boolean;
  /** Help under the box, from the STOXXO manual's notes on that field. */
  hint?: string;
}

const AUTO_ONLY = "Used only for automatic sign-in.";
const PIN_HINT = "Used only for automatic sign-in. If the broker asks several security questions, answer them all the same and enter that answer here.";
const TOTP_HINT = "The secret key from the authenticator setup, not the 6-digit code. Used only for automatic sign-in.";

/** What each broker needs to sign in, taken from the engine's own rules. */
export const BROKERS: Record<BrokerId, { name: string; fields: Field[]; note: string }> = {
  zerodha: {
    name: "Zerodha",
    note: "Register the outbound IP with Zerodha before placing orders.",
    fields: [
      { key: "userId", label: "User ID", secret: false },
      { key: "apiKey", label: "API key", secret: true },
      { key: "apiSecret", label: "API secret", secret: true },
      { key: "password", label: "Web password", secret: true, hint: AUTO_ONLY },
      { key: "totpSecret", label: "TOTP secret", secret: true, hint: TOTP_HINT },
    ],
  },
  angel: {
    name: "Angel One",
    note: "Use the key from your SmartAPI app.",
    fields: [
      { key: "userId", label: "Client code", secret: false },
      { key: "apiKey", label: "API key", secret: true },
      { key: "pin", label: "PIN", secret: true, hint: PIN_HINT },
      { key: "totpSecret", label: "TOTP secret", secret: true, hint: TOTP_HINT },
    ],
  },
  kotak: {
    name: "Kotak Neo",
    note: "The consumer key is the long-lived token from the Neo API page.",
    fields: [
      { key: "apiKey", label: "Consumer key", secret: true },
      { key: "userId", label: "UCC", secret: false },
      { key: "mobile", label: "Mobile", secret: false },
      { key: "pin", label: "MPIN", secret: true, hint: PIN_HINT },
      { key: "totpSecret", label: "TOTP secret", secret: true, hint: TOTP_HINT },
    ],
  },
  upstox: {
    name: "Upstox",
    note: "The redirect URL must match the one in your Upstox app.",
    fields: [
      { key: "apiKey", label: "API key", secret: true },
      { key: "apiSecret", label: "API secret", secret: true },
      { key: "redirectUrl", label: "Redirect URL", secret: false },
      { key: "mobile", label: "Mobile", secret: false },
      { key: "pin", label: "PIN", secret: true, hint: PIN_HINT },
      { key: "totpSecret", label: "TOTP secret", secret: true, hint: TOTP_HINT },
    ],
  },
  nubra: {
    name: "Nubra",
    note: "Without a TOTP secret, Nubra sends an SMS code the first time on each computer.",
    fields: [
      { key: "mobile", label: "Mobile", secret: false },
      { key: "pin", label: "MPIN", secret: true, hint: PIN_HINT },
      { key: "totpSecret", label: "TOTP secret (optional)", secret: true, hint: TOTP_HINT },
    ],
  },
  paper: {
    name: "Simulator",
    note: "Uses live prices and never sends a real order.",
    fields: [],
  },
};

export const OUTBOUND = ["No network", "VPS IP 1", "VPS IP 2", "VPS IP 3", "Proxy"];

/** Per-account limits. 0 (or blank time) means off. Rupee amounts are of MTM. */
export interface Risk {
  maxProfit: number;
  maxLoss: number;
  /** "If profit reaches lockAt, lock at least lockMin". */
  lockAt: number;
  lockMin: number;
  /** "For every increase of trailEvery, trail profit by trailBy". */
  trailEvery: number;
  trailBy: number;
  maxOpenTrades: number;
  maxLossPerTrade: number;
  qtyOnMaxLoss: boolean;
  /** HH:MM or HH:MM:SS. At this time all positions are closed. */
  sqOffTime: string;
  /** Also close positional (NRML) positions at square-off. */
  sqOffNrml: boolean;
}

export interface AlertSettings {
  telegramIds: string;
  /** Email alerts. Stored only: not sent yet. */
  email: string;
  /** Ask for approval on Telegram before each trade. */
  authRequired: boolean;
}

export const noRisk = (): Risk => ({
  maxProfit: 0, maxLoss: 0, lockAt: 0, lockMin: 0, trailEvery: 0, trailBy: 0,
  maxOpenTrades: 0, maxLossPerTrade: 0, qtyOnMaxLoss: false, sqOffTime: "", sqOffNrml: false,
});

export interface Account {
  id: string;
  on: boolean;
  status: Status;
  alias: string;
  broker: BrokerId;
  userId: string;
  available: number;
  mtm: number;
  marketOrders: boolean;
  slPct: number;
  /** What to do when a stop-loss cannot be placed. */
  slFailure: "exit" | "alert";
  autoLogin: boolean;
  outbound: string;
  /** Scales every order's quantity (0.5 halves it, 2 doubles it), in whole lots. */
  qtyMultiplier: number;
  /** Rupees per trade; quantity = this / price, rounded to whole lots. 0 = off. */
  exposure: number;
  risk: Risk;
  alerts: AlertSettings;
  /** Which secret fields are already saved. The values are never held here. */
  saved: Record<string, boolean>;
  /** Non-secret sign-in details other than the user ID (mobile, redirect URL). */
  fields: Record<string, string>;
  /** MTM split by product, shown beside the total. */
  mtmMis: number;
  mtmNrml: number;
  /** The broker's historical (candle) API is available on this account. */
  historicalApi: boolean;
}

const allSaved = (b: BrokerId) => Object.fromEntries(BROKERS[b].fields.filter((f) => f.secret).map((f) => [f.key, true]));

// Sample rows until the engine has an accounts store (next build step).
export const SAMPLE: Account[] = [
  { id: "a1", on: true, status: "in", alias: "Self", broker: "zerodha", userId: "AB1234", available: 5448697, mtm: 1240, mtmMis: 1240, mtmNrml: 0, marketOrders: false, slPct: 50, slFailure: "exit", autoLogin: true, outbound: "VPS IP 1", qtyMultiplier: 1, exposure: 0, risk: { ...noRisk(), maxLoss: 15000, maxOpenTrades: 4, sqOffTime: "15:15:00", sqOffNrml: false }, alerts: { telegramIds: "12001200", email: "", authRequired: false }, saved: allSaved("zerodha"), fields: {}, historicalApi: true },
  { id: "a2", on: true, status: "in", alias: "Papa", broker: "angel", userId: "XY5678", available: 1748510, mtm: 0, mtmMis: 0, mtmNrml: 0, marketOrders: true, slPct: 50, slFailure: "exit", autoLogin: true, outbound: "VPS IP 2", qtyMultiplier: 2, exposure: 0, risk: { ...noRisk(), maxLoss: 10000, maxProfit: 25000, lockAt: 10000, lockMin: 5000 }, alerts: { telegramIds: "", email: "", authRequired: false }, saved: allSaved("angel"), fields: {}, historicalApi: false },
  { id: "a3", on: true, status: "need", alias: "Wife", broker: "kotak", userId: "K90210", available: 0, mtm: 0, mtmMis: 0, mtmNrml: 0, marketOrders: true, slPct: 60, slFailure: "alert", autoLogin: true, outbound: "VPS IP 3", qtyMultiplier: 0.5, exposure: 0, risk: noRisk(), alerts: { telegramIds: "", email: "", authRequired: false }, saved: allSaved("kotak"), fields: { mobile: "98xxxxxx10" }, historicalApi: false },
  { id: "a4", on: false, status: "sim", alias: "Paper", broker: "paper", userId: "SIM1", available: 1000000, mtm: 0, mtmMis: 0, mtmNrml: 0, marketOrders: true, slPct: 50, slFailure: "exit", autoLogin: false, outbound: "No network", qtyMultiplier: 1, exposure: 0, risk: noRisk(), alerts: { telegramIds: "", email: "", authRequired: false }, saved: {}, fields: {}, historicalApi: false },
];

export const accounts = signal<Account[]>(SAMPLE);
export const openId = signal<string | null>(null);
export const toast = signal<string>("");

export type EngineState =
  | { state: "checking" }
  | { state: "down" }
  | { state: "up"; broker: string; ms: number };
export const engine = signal<EngineState>({ state: "checking" });
export const engineUrl = signal<string>("http://127.0.0.1:8088");

let toastTimer: number | undefined;
export function showToast(msg: string) {
  toast.value = msg;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.value = ""), 2200);
}

export function updateAccount(id: string, patch: Partial<Account>) {
  accounts.value = accounts.value.map((a) => (a.id === id ? { ...a, ...patch } : a));
}

export function addAccount() {
  const id = "a" + Date.now().toString(36);
  const a: Account = {
    id, on: false, status: "off", alias: "New account", broker: "zerodha", userId: "", available: 0, mtm: 0, mtmMis: 0, mtmNrml: 0,
    marketOrders: false, slPct: 50, slFailure: "exit", autoLogin: true, outbound: "VPS IP 1", qtyMultiplier: 1, exposure: 0,
    risk: noRisk(), alerts: { telegramIds: "", email: "", authRequired: false }, saved: {}, fields: {}, historicalApi: false,
  };
  accounts.value = [...accounts.value, a];
  openId.value = id;
}

export function removeAccount(id: string) {
  accounts.value = accounts.value.filter((a) => a.id !== id);
  if (openId.value === id) openId.value = null;
}

export type Theme = "dark" | "light" | "system";

function loadTheme(): Theme {
  try {
    const v = localStorage.getItem("theme");
    if (v === "dark" || v === "light" || v === "system") return v;
  } catch {
    /* storage can be unavailable: fall back to dark */
  }
  return "dark";
}

/** Dark is the default; the choice is remembered on this computer. */
export const theme = signal<Theme>(loadTheme());

export function setTheme(t: Theme) {
  theme.value = t;
  document.documentElement.dataset.theme = t;
  try {
    localStorage.setItem("theme", t);
  } catch {
    /* not remembered, still applied */
  }
}

/** The accounts grid's search, broker and status filters, and its sort. */
export const filters = signal<Filters>(noFilters());
export const sortState = signal<Sort>({ key: null, dir: "asc" });

export function setEnabled(ids: string[], on: boolean) {
  const set = new Set(ids);
  accounts.value = accounts.value.map((a) => (set.has(a.id) ? { ...a, on } : a));
}

/** True when the window is wide enough for the MIS and NRML MTM columns. */
const wideQuery = window.matchMedia("(min-width: 1101px)");
export const wide = signal(wideQuery.matches);
wideQuery.addEventListener("change", (e) => (wide.value = e.matches));

import { signal } from "@preact/signals";
import { noFilters, type Filters, type Sort } from "./grid";

export type BrokerId = "zerodha" | "angel" | "kotak" | "upstox" | "nubra" | "paper";
export type Status = "in" | "need" | "off" | "sim";

/** The sign-in columns of the table. A broker fills the ones it needs and the rest show a dash. */
export type CredCol = "userId" | "apiKey" | "apiSecret" | "login" | "totp" | "mobile" | "redirect";

export interface Field {
  key: string;
  label: string;
  /** Secret fields are write-only: once saved they are never shown again. */
  secret: boolean;
  /** Which table column this goes in. */
  col: CredCol;
  /** Where to find it. Shown as the cell's hint while adding the account. */
  where: string;
}

export const CRED_COLS: { id: CredCol; label: string; w: number }[] = [
  { id: "userId", label: "User ID", w: 100 },
  { id: "apiKey", label: "API key", w: 124 },
  { id: "apiSecret", label: "API secret", w: 112 },
  { id: "login", label: "Password / PIN", w: 112 },
  { id: "totp", label: "TOTP secret", w: 112 },
  { id: "mobile", label: "Mobile", w: 120 },
  { id: "redirect", label: "Redirect URL", w: 150 },
];

/** What each broker needs to sign in, and where the user finds each thing. */
export const BROKERS: Record<BrokerId, { name: string; fields: Field[]; note: string }> = {
  zerodha: {
    name: "Zerodha",
    note: "Register the outbound IP with Zerodha before placing orders. The first time, approve the app once on Kite's Authorize page.",
    fields: [
      { key: "userId", label: "User ID", secret: false, col: "userId", where: "Your Zerodha client ID, like AB1234." },
      { key: "apiKey", label: "API key", secret: true, col: "apiKey", where: "From your app on the Kite Connect developer console (developers.kite.trade)." },
      { key: "apiSecret", label: "API secret", secret: true, col: "apiSecret", where: "Shown with the API key on the same app page." },
      { key: "password", label: "Web password", secret: true, col: "login", where: "The password you use to log in to kite.zerodha.com." },
      { key: "totpSecret", label: "TOTP secret", secret: true, col: "totp", where: "On Kite, turn on External TOTP and copy the key under \"Can't scan? Copy key\". Not the 6-digit code." },
    ],
  },
  angel: {
    name: "Angel One",
    note: "Use the key from your SmartAPI app.",
    fields: [
      { key: "userId", label: "Client code", secret: false, col: "userId", where: "Your Angel One client code, like A123456." },
      { key: "apiKey", label: "API key", secret: true, col: "apiKey", where: "From your app on the SmartAPI portal (smartapi.angelone.in)." },
      { key: "pin", label: "PIN", secret: true, col: "login", where: "Your Angel One login PIN." },
      { key: "totpSecret", label: "TOTP secret", secret: true, col: "totp", where: "Enable TOTP on the SmartAPI portal and copy the secret key shown. Not the 6-digit code." },
    ],
  },
  kotak: {
    name: "Kotak Neo",
    note: "Each Kotak user has their own consumer key and their own TOTP registration.",
    fields: [
      { key: "userId", label: "UCC", secret: false, col: "userId", where: "Your UCC, from your Neo profile." },
      { key: "apiKey", label: "Consumer key", secret: true, col: "apiKey", where: "The token on your own Neo API page. Every user has a different one." },
      { key: "pin", label: "MPIN", secret: true, col: "login", where: "Your 6-digit Neo MPIN." },
      { key: "totpSecret", label: "TOTP secret", secret: true, col: "totp", where: "The key shown when you register TOTP in the Neo app. Finish by entering the first 6-digit code there." },
      { key: "mobile", label: "Mobile", secret: false, col: "mobile", where: "Your registered mobile with +91, like +919876543210." },
    ],
  },
  upstox: {
    name: "Upstox",
    note: "The account must use TOTP, not SMS OTP, as its second factor.",
    fields: [
      { key: "apiKey", label: "API key", secret: true, col: "apiKey", where: "From your app on the Upstox developer console." },
      { key: "apiSecret", label: "API secret", secret: true, col: "apiSecret", where: "Shown with the API key on the same app page." },
      { key: "pin", label: "PIN", secret: true, col: "login", where: "Your 6-digit Upstox PIN." },
      { key: "totpSecret", label: "TOTP secret", secret: true, col: "totp", where: "Turn on TOTP in the Upstox app and copy the secret key. SMS OTP will not work." },
      { key: "mobile", label: "Mobile", secret: false, col: "mobile", where: "The mobile number on your Upstox account." },
      { key: "redirectUrl", label: "Redirect URL", secret: false, col: "redirect", where: "Exactly as registered in your Upstox app, character for character." },
    ],
  },
  nubra: {
    name: "Nubra",
    note: "Turn on TOTP in the Nubra app first.",
    fields: [
      { key: "pin", label: "MPIN", secret: true, col: "login", where: "Your Nubra MPIN." },
      { key: "totpSecret", label: "TOTP secret", secret: true, col: "totp", where: "Turn on TOTP in the Nubra app and copy the secret key." },
      { key: "mobile", label: "Mobile", secret: false, col: "mobile", where: "The mobile number on your Nubra account." },
    ],
  },
  paper: {
    name: "Simulator",
    note: "Uses live prices and never sends a real order.",
    fields: [],
  },
};

/** The field a broker keeps in a table column, or undefined when it does not use that column. */
export const fieldFor = (b: BrokerId, col: CredCol): Field | undefined => BROKERS[b].fields.find((f) => f.col === col);

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

/** An account's margin in the same terms for every broker. `available` includes pledged collateral. */
export interface Funds {
  available: number;
  used: number;
  collateral: number;
  total: number;
  /** The cash part on its own, where the broker reports it. */
  cash: number | null;
  /** The broker's own further lines, label and amount, in display order. */
  details: [string, number][];
}

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
  /** Why the last sign-in failed, as the engine reported it. Blank when fine. */
  signInError: string;
  /** The margin the broker reported at the last read; null before the first. */
  funds: Funds | null;
  /** Why the last margin read failed, when it did. Blank when fine. */
  fundsError: string;
  /** Added in the app and not saved to the engine yet. */
  unsaved?: boolean;
}

/** Filled from the engine, which holds the accounts. */
export const accounts = signal<Account[]>([]);
export const accountsLoaded = signal(false);
/** True while Login all is running. */
export const busyAll = signal(false);
/** True while margins are being read. */
export const busyFunds = signal(false);
/** Edits typed into the table's cells and not saved yet, by account id. */
export interface Draft {
  alias?: string;
  broker?: BrokerId;
  /** Only the cells the user touched, by field key. A secret is held here only while it is being typed. */
  vals: Record<string, string>;
  /** Field keys (and "alias") that were missing when the user tried to save. */
  invalid: string[];
}
export const drafts = signal<Record<string, Draft>>({});

/** Show the sign-in values as text: what is being typed, and the saved values read back from the engine. */
export const revealTyped = signal(false);
/** Saved values read back for display, by account id then field key. Emptied when hidden. */
export const revealed = signal<Record<string, Record<string, string>>>({});
/** Seconds until the shown values hide again; 0 while hidden. */
export const revealLeft = signal(0);

/** The account being added, shown as the first row. Not in the list until it is saved. */
export const newAccount = signal<Account | null>(null);

/** What the engine has for a non-secret field. */
export function currentVal(a: Account, key: string): string {
  return key === "userId" ? a.userId : a.fields[key] ?? "";
}

export function patchDraft(id: string, p: Partial<Omit<Draft, "vals">> & { val?: [string, string] }) {
  const old = drafts.value[id] ?? { vals: {}, invalid: [] };
  const vals = p.val ? { ...old.vals, [p.val[0]]: p.val[1] } : old.vals;
  const { val: _v, ...rest } = p;
  // Typing in a flagged cell clears its flag.
  const invalid = p.val ? old.invalid.filter((k) => k !== p.val![0]) : old.invalid;
  drafts.value = { ...drafts.value, [id]: { ...old, ...rest, vals, invalid } };
}

export function dropDraft(id: string) {
  const { [id]: _gone, ...rest } = drafts.value;
  drafts.value = rest;
}

/** True when the row has something typed that is not saved: a new row, or any changed cell. */
export function isDirty(a: Account): boolean {
  if (a.unsaved) return true;
  const d = drafts.value[a.id];
  if (!d) return false;
  if (d.alias !== undefined && d.alias.trim() !== a.alias) return true;
  if (d.broker !== undefined && d.broker !== a.broker) return true;
  return Object.entries(d.vals).some(([k, v]) => {
    const f = BROKERS[d.broker ?? a.broker].fields.find((x) => x.key === k);
    return f?.secret ? v.trim() !== "" : v.trim() !== currentVal(a, k);
  });
}
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

/** Changes a row and saves the change to the engine (secrets are not touched). */
export function updateAccount(id: string, patch: Partial<Account>) {
  accounts.value = accounts.value.map((a) => (a.id === id ? { ...a, ...patch } : a));
  const a = accounts.value.find((x) => x.id === id);
  // A row that was never saved is saved by its own Save button, not half-way.
  if (a && !a.unsaved) {
    persist(a).catch((e) => showToast(`Not saved: ${String(e)}`));
  }
}

let persist: (a: Account) => Promise<unknown> = async () => {};
/** Set by the engine layer so this file stays free of Tauri imports. */
export function setPersist(f: (a: Account) => Promise<unknown>) {
  persist = f;
}

/** Adds a blank row at the top of the table. Nothing is saved until the row is. */
export function addAccount() {
  const id = "a" + Date.now().toString(36);
  newAccount.value = {
    id, on: true, status: "need", alias: "", broker: "zerodha", userId: "", available: 0, mtm: 0, mtmMis: 0, mtmNrml: 0,
    marketOrders: false, slPct: 50, slFailure: "exit", autoLogin: true, outbound: "VPS IP 1", qtyMultiplier: 1, exposure: 0,
    risk: noRisk(), alerts: { telegramIds: "", email: "", authRequired: false }, saved: {}, fields: {}, historicalApi: false,
    signInError: "", funds: null, fundsError: "", unsaved: true,
  };
}

export function removeAccount(id: string) {
  accounts.value = accounts.value.filter((a) => a.id !== id);
  dropDraft(id);
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

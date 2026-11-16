// Pure checks for the account form. No UI imports, so they can be tested alone
// (`node --test src/validate.test.ts`).

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

interface NumOpts {
  /** A blank field is an error instead of "off" (0). */
  required?: boolean;
  /** Must be above zero. */
  positive?: boolean;
  int?: boolean;
  max?: number;
}

/** Read a number from a text box. Blank means "off" (0) unless required. */
export function num(label: string, text: string, o: NumOpts = {}): Parsed<number> {
  const t = text.replace(/[,\s₹]/g, "");
  if (t === "") {
    return o.required ? { ok: false, error: `Enter ${label}` } : { ok: true, value: 0 };
  }
  const n = Number(t);
  if (!Number.isFinite(n)) return { ok: false, error: `${label} must be a number` };
  if (n < 0) return { ok: false, error: `${label} can't be negative` };
  if (o.positive && n <= 0) return { ok: false, error: `${label} must be above zero` };
  if (o.int && !Number.isInteger(n)) return { ok: false, error: `${label} must be a whole number` };
  if (o.max !== undefined && n > o.max) return { ok: false, error: `${label} can't be above ${o.max}` };
  return { ok: true, value: n };
}

/** Blank, HH:MM or HH:MM:SS on a 24-hour clock. */
export function validTime(s: string): boolean {
  if (s.trim() === "") return true;
  const m = /^(\d{1,2}):(\d{2})(?::(\d{2}))?$/.exec(s.trim());
  if (!m) return false;
  const [h, min, sec] = [Number(m[1]), Number(m[2]), Number(m[3] ?? 0)];
  return h <= 23 && min <= 59 && sec <= 59;
}

/** Blank, or something that looks like an email address. */
export function validEmail(s: string): boolean {
  return s.trim() === "" || /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s.trim());
}

/**
 * The square-off time as it should be stored. Blank and 00:00:00 both mean
 * "off", as in STOXXO, so they are stored as blank.
 */
export function normalizeSqOff(s: string): string {
  const t = s.trim();
  return /^0{1,2}:00(:00)?$/.test(t) ? "" : t;
}

/** Blank, or comma-separated whole numbers (groups and channels are negative). */
export function validTelegram(s: string): boolean {
  if (s.trim() === "") return true;
  return s.split(",").every((p) => /^-?\d{5,}$/.test(p.trim()));
}

export interface Limits {
  maxProfit: number;
  maxLoss: number;
  lockAt: number;
  lockMin: number;
  trailEvery: number;
  trailBy: number;
  maxLossPerTrade: number;
  qtyOnMaxLoss: boolean;
  authRequired: boolean;
  telegramIds: string;
}

/** Rules that involve more than one field. Returns the first problem, or null. */
export function checkLimits(l: Limits): string | null {
  if (l.lockAt > 0 && l.lockMin <= 0) return "Enter the profit to lock, or clear 'If profit reaches'";
  if (l.lockAt <= 0 && l.lockMin > 0) return "Enter 'If profit reaches', or clear the profit to lock";
  if (l.lockAt > 0 && l.lockMin >= l.lockAt) return "The locked profit must be lower than the profit that triggers it";
  if (l.trailEvery > 0 && l.trailBy <= 0) return "Enter how much to trail by, or clear 'For every increase'";
  if (l.trailEvery <= 0 && l.trailBy > 0) return "Enter 'For every increase', or clear 'Trail by'";
  if (l.trailEvery > 0 && l.trailBy > l.trailEvery) return "Trail by an amount smaller than the increase, so some gap is kept";
  if (l.maxProfit > 0 && l.lockAt > l.maxProfit) return "'If profit reaches' is above the max profit, so it would never apply";
  if (l.qtyOnMaxLoss && l.maxLossPerTrade <= 0) return "Enter a max loss per trade to size quantity from it";
  if (l.authRequired && l.telegramIds.trim() === "") return "Enter a Telegram ID to approve trades there";
  return null;
}

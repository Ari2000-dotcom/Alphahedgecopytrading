// Per-column filters for the accounts table. Pure, so it is tested alone
// (`node --test src/colfilter.test.ts`).

/** What a filter needs to know about one row, already read out of the account. */
export interface FilterRow {
  on: boolean;
  auto: boolean;
  alias: string;
  broker: string;
  status: "in" | "need" | "failed" | "sim";
  available: number | null;
  used: number | null;
  total: number | null;
  cash: number | null;
  collateral: number | null;
  /** By sign-in column id: does this broker use it, is something saved, and the text (for plain fields). */
  cred: Record<string, { used: boolean; saved: boolean; text: string }>;
}

/** Sign-in columns that hold a secret: they filter by saved / missing, never by text. */
export const SECRET_COLS = new Set(["apiKey", "apiSecret", "login", "totp"]);

const has = (text: string, q: string) => text.toLowerCase().includes(q.trim().toLowerCase());

/**
 * A number against what was typed: "1000" or "=1000" (equal), ">1000", ">=1000",
 * "<500", "<=500". Commas are ignored. Anything else is matched as text, so
 * "202" still finds 202.87.
 */
export function numberMatch(value: number | null, q: string): boolean {
  const t = q.trim();
  if (!t) return true;
  if (value === null) return false;
  const m = /^(>=|<=|>|<|=)?\s*(-?[\d,]*\.?\d+)$/.exec(t);
  if (!m) return has(value.toFixed(2), t);
  const n = Number(m[2].replace(/,/g, ""));
  switch (m[1]) {
    case ">": return value > n;
    case ">=": return value >= n;
    case "<": return value < n;
    case "<=": return value <= n;
    case "=": return Math.abs(value - n) < 0.005;
    default: return value.toFixed(2).includes(m[2].replace(/,/g, ""));
  }
}

/** True when the row passes the filter typed or picked for one column. A blank filter passes everything. */
export function matches(r: FilterRow, col: string, q: string): boolean {
  if (!q.trim()) return true;
  switch (col) {
    case "on": return q === "on" ? r.on : !r.on;
    case "auto": return q === "on" ? r.auto : !r.auto;
    case "name": return has(r.alias, q);
    case "broker": return r.broker === q;
    case "status": return r.status === q;
    case "available": return numberMatch(r.available, q);
    case "used": return numberMatch(r.used, q);
    case "total": return numberMatch(r.total, q);
    case "cash": return numberMatch(r.cash, q);
    case "collateral": return numberMatch(r.collateral, q);
  }
  if (col.startsWith("cred:")) {
    const id = col.slice(5);
    const c = r.cred[id];
    if (!c) return false;
    if (SECRET_COLS.has(id)) {
      if (q === "saved") return c.used && c.saved;
      if (q === "missing") return c.used && !c.saved;
      if (q === "na") return !c.used;
      return true;
    }
    return c.used && has(c.text, q);
  }
  return true;
}

/** Every column filter at once. */
export function passes(r: FilterRow, filters: Record<string, string>): boolean {
  return Object.entries(filters).every(([col, q]) => matches(r, col, q));
}

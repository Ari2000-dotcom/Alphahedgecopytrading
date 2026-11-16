// Filtering and sorting for the accounts grid. Pure, so it is tested alone
// (`node --test src/grid.test.ts`).

export type SortKey = "alias" | "broker" | "userId" | "available" | "mtm" | "mtmMis" | "mtmNrml";
export type SortDir = "asc" | "desc";

export interface GridRow {
  alias: string;
  broker: string;
  userId: string;
  status: string;
  available: number;
  mtm: number;
  mtmMis: number;
  mtmNrml: number;
}

export interface Filters {
  search: string;
  /** "all" or a broker id */
  broker: string;
  /** "all", "in", "need", "off" or "sim" */
  status: string;
}

export interface Sort {
  key: SortKey | null;
  dir: SortDir;
}

export const noFilters = (): Filters => ({ search: "", broker: "all", status: "all" });

export function filtersActive(f: Filters): boolean {
  return f.search.trim() !== "" || f.broker !== "all" || f.status !== "all";
}

/** The next sort when a header is clicked: ascending, then descending, then off. */
export function nextSort(s: Sort, key: SortKey): Sort {
  if (s.key !== key) return { key, dir: "asc" };
  if (s.dir === "asc") return { key, dir: "desc" };
  return { key: null, dir: "asc" };
}

export function viewRows<T extends GridRow>(
  rows: T[],
  f: Filters,
  s: Sort,
  brokerName: (id: string) => string,
): T[] {
  const q = f.search.trim().toLowerCase();
  let out = rows.filter((r) => {
    if (f.broker !== "all" && r.broker !== f.broker) return false;
    if (f.status !== "all" && r.status !== f.status) return false;
    if (!q) return true;
    return [r.alias, r.userId, brokerName(r.broker)].some((t) => t.toLowerCase().includes(q));
  });
  if (s.key) {
    const k = s.key;
    const sign = s.dir === "asc" ? 1 : -1;
    // Array.prototype.sort is stable, so equal rows keep their order.
    out = [...out].sort((a, b) => {
      const x = k === "broker" ? brokerName(a.broker) : a[k];
      const y = k === "broker" ? brokerName(b.broker) : b[k];
      if (typeof x === "number" && typeof y === "number") return (x - y) * sign;
      return String(x).localeCompare(String(y), undefined, { sensitivity: "base", numeric: true }) * sign;
    });
  }
  return out;
}

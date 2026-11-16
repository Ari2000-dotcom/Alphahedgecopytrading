import { test } from "node:test";
import assert from "node:assert/strict";
import { filtersActive, nextSort, noFilters, viewRows, type GridRow } from "./grid.ts";

const name = (id: string) => ({ zerodha: "Zerodha", angel: "Angel One", kotak: "Kotak Neo" })[id] ?? id;
const row = (alias: string, broker: string, status: string, available: number, mtm: number, userId = alias + "01"): GridRow =>
  ({ alias, broker, userId, status, available, mtm, mtmMis: mtm, mtmNrml: 0 });

const rows = [
  row("Self", "zerodha", "in", 5448697, 1240, "AB1234"),
  row("Papa", "angel", "in", 1748510, 0, "XY5678"),
  row("Wife", "kotak", "need", 0, -300, "K90210"),
  row("Paper", "paper", "sim", 1000000, 0, "SIM1"),
];
const none = { key: null, dir: "asc" } as const;

test("no filters, no sort: rows come back unchanged", () => {
  assert.deepEqual(viewRows(rows, noFilters(), none, name).map((r) => r.alias), ["Self", "Papa", "Wife", "Paper"]);
});

test("search matches alias, user id and broker name, ignoring case", () => {
  const f = (search: string) => viewRows(rows, { ...noFilters(), search }, none, name).map((r) => r.alias);
  assert.deepEqual(f("pa"), ["Papa", "Paper"]);
  assert.deepEqual(f("ab12"), ["Self"]);
  assert.deepEqual(f("kotak"), ["Wife"]);
  assert.deepEqual(f("  ANGEL "), ["Papa"]);
  assert.deepEqual(f("zzz"), []);
});

test("broker and status filters combine", () => {
  assert.deepEqual(viewRows(rows, { ...noFilters(), status: "in" }, none, name).map((r) => r.alias), ["Self", "Papa"]);
  assert.deepEqual(viewRows(rows, { ...noFilters(), broker: "angel", status: "in" }, none, name).map((r) => r.alias), ["Papa"]);
  assert.deepEqual(viewRows(rows, { ...noFilters(), broker: "angel", status: "need" }, none, name), []);
});

test("numeric sort, both ways, negatives included", () => {
  assert.deepEqual(viewRows(rows, noFilters(), { key: "mtm", dir: "asc" }, name).map((r) => r.alias), ["Wife", "Papa", "Paper", "Self"]);
  assert.deepEqual(viewRows(rows, noFilters(), { key: "available", dir: "desc" }, name).map((r) => r.alias), ["Self", "Papa", "Paper", "Wife"]);
});

test("text sort uses the broker's display name", () => {
  assert.deepEqual(viewRows(rows, noFilters(), { key: "broker", dir: "asc" }, name).map((r) => r.broker), ["angel", "kotak", "paper", "zerodha"]);
});

test("sorting never changes the input and keeps equal rows in order", () => {
  const copy = rows.map((r) => r.alias);
  viewRows(rows, noFilters(), { key: "mtm", dir: "desc" }, name);
  assert.deepEqual(rows.map((r) => r.alias), copy);
  const out = viewRows(rows, noFilters(), { key: "mtm", dir: "asc" }, name).map((r) => r.alias);
  assert.ok(out.indexOf("Papa") < out.indexOf("Paper"), "equal MTM keeps the original order");
});

test("header clicks cycle ascending, descending, off", () => {
  let s = nextSort(none, "alias");
  assert.deepEqual(s, { key: "alias", dir: "asc" });
  s = nextSort(s, "alias");
  assert.deepEqual(s, { key: "alias", dir: "desc" });
  s = nextSort(s, "alias");
  assert.deepEqual(s, { key: null, dir: "asc" });
  assert.deepEqual(nextSort({ key: "alias", dir: "desc" }, "mtm"), { key: "mtm", dir: "asc" });
});

test("filtersActive", () => {
  assert.equal(filtersActive(noFilters()), false);
  assert.equal(filtersActive({ ...noFilters(), search: " " }), false);
  assert.equal(filtersActive({ ...noFilters(), broker: "angel" }), true);
});

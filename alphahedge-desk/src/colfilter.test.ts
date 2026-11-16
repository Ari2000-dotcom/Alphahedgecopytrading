import { test } from "node:test";
import assert from "node:assert/strict";
import { matches, numberMatch, passes, type FilterRow } from "./colfilter.ts";

const row = (p: Partial<FilterRow> = {}): FilterRow => ({
  on: true, auto: true, alias: "Ravi", broker: "kotak", status: "in",
  available: 202.87, used: 0, total: 202.87, cash: 40.5, collateral: 162.37,
  cred: {
    userId: { used: true, saved: true, text: "K77AB" },
    apiKey: { used: true, saved: true, text: "" },
    apiSecret: { used: false, saved: false, text: "" },
    totp: { used: true, saved: false, text: "" },
    mobile: { used: true, saved: true, text: "+919800000001" },
  },
  ...p,
});

test("a blank filter passes everything", () => {
  assert.ok(matches(row(), "name", ""));
  assert.ok(matches(row(), "available", "   "));
  assert.ok(passes(row(), { name: "", status: "" }));
});

test("text columns match any part, ignoring case", () => {
  assert.ok(matches(row(), "name", "rav"));
  assert.ok(!matches(row(), "name", "papa"));
  assert.ok(matches(row(), "cred:userId", "7ab"));
  assert.ok(matches(row(), "cred:mobile", "98000"));
});

test("numbers take comparisons, or match as text", () => {
  assert.ok(numberMatch(202.87, ">200"));
  assert.ok(!numberMatch(202.87, ">300"));
  assert.ok(numberMatch(202.87, "<=202.87"));
  assert.ok(numberMatch(1234567.5, ">1,000,000"));
  assert.ok(numberMatch(202.87, "=202.87"));
  assert.ok(numberMatch(202.87, "202"));
  assert.ok(!numberMatch(null, ">0"), "no margin read yet never passes a number filter");
});

test("secret columns filter by saved, missing or not used, never by value", () => {
  assert.ok(matches(row(), "cred:apiKey", "saved"));
  assert.ok(!matches(row(), "cred:apiKey", "missing"));
  assert.ok(matches(row(), "cred:totp", "missing"));
  assert.ok(matches(row(), "cred:apiSecret", "na"));
  assert.ok(!matches(row(), "cred:apiSecret", "saved"));
});

test("pick lists: active, auto, broker, status", () => {
  assert.ok(matches(row(), "on", "on"));
  assert.ok(!matches(row({ on: false }), "on", "on"));
  assert.ok(matches(row({ auto: false }), "auto", "off"));
  assert.ok(matches(row(), "broker", "kotak"));
  assert.ok(!matches(row(), "broker", "zerodha"));
  assert.ok(matches(row({ status: "failed" }), "status", "failed"));
});

test("several filters must all pass", () => {
  assert.ok(passes(row(), { broker: "kotak", available: ">100" }));
  assert.ok(!passes(row(), { broker: "kotak", available: ">1000" }));
});

test("cash and collateral filter like the other figures", () => {
  assert.ok(matches(row(), "collateral", ">100"));
  assert.ok(!matches(row(), "cash", ">100"));
  assert.ok(!matches(row({ cash: null }), "cash", ">0"), "a broker that does not report cash never passes");
});

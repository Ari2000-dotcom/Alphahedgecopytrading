import { test } from "node:test";
import assert from "node:assert/strict";
import { moneyParts, moneyText } from "./format.ts";

test("indian grouping with two decimals", () => {
  assert.equal(moneyText(5448697), "54,48,697.00");
  assert.equal(moneyText(1748510.5), "17,48,510.50");
  assert.equal(moneyText(10000000), "1,00,00,000.00");
  assert.equal(moneyText(999), "999.00");
  assert.equal(moneyText(1000), "1,000.00");
});

test("signs", () => {
  assert.equal(moneyText(1240, true), "+1,240.00");
  assert.equal(moneyText(1240), "1,240.00");
  assert.equal(moneyText(-1240.5), "-1,240.50");
  assert.equal(moneyText(-1240.5, true), "-1,240.50");
});

test("zero and tiny values are neutral and never show -0.00", () => {
  assert.equal(moneyText(0, true), "0.00");
  assert.equal(moneyText(-0.004, true), "0.00");
  assert.equal(moneyParts(-0.004)?.tone, "zero");
  assert.equal(moneyParts(0.004)?.tone, "zero");
  assert.equal(moneyParts(-0.006)?.sign, "-");
});

test("rounding at the cent boundary carries into the whole part", () => {
  assert.equal(moneyText(99.995), "100.00");
  assert.equal(moneyText(1.005 * 100 / 100 + 0.0049), "1.01");
});

test("tone follows the value", () => {
  assert.equal(moneyParts(5)?.tone, "pos");
  assert.equal(moneyParts(-5)?.tone, "neg");
  assert.equal(moneyParts(0)?.tone, "zero");
});

test("non-numbers are not shown as money", () => {
  assert.equal(moneyParts(NaN), null);
  assert.equal(moneyText(Infinity), "–");
});

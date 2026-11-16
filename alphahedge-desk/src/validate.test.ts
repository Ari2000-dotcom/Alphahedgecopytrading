import { test } from "node:test";
import assert from "node:assert/strict";
import { checkLimits, normalizeSqOff, num, validEmail, validTelegram, validTime, type Limits } from "./validate.ts";

const base: Limits = {
  maxProfit: 0, maxLoss: 0, lockAt: 0, lockMin: 0, trailEvery: 0, trailBy: 0,
  maxLossPerTrade: 0, qtyOnMaxLoss: false, authRequired: false, telegramIds: "",
};

test("blank numbers mean off, unless required", () => {
  assert.deepEqual(num("Max loss", ""), { ok: true, value: 0 });
  assert.equal(num("Multiplier", "", { required: true }).ok, false);
});

test("numbers accept commas and the rupee sign, refuse bad input", () => {
  assert.deepEqual(num("Max loss", "₹ 5,000"), { ok: true, value: 5000 });
  assert.equal(num("Max loss", "abc").ok, false);
  assert.equal(num("Max loss", "-5").ok, false);
  assert.equal(num("Multiplier", "0", { positive: true }).ok, false);
  assert.deepEqual(num("Multiplier", "0.5", { positive: true }), { ok: true, value: 0.5 });
  assert.equal(num("Trades", "2.5", { int: true }).ok, false);
  assert.equal(num("SL", "120", { max: 99 }).ok, false);
});

test("times", () => {
  for (const ok of ["", "15:15", "15:15:00", "9:30", "00:00:00"]) assert.ok(validTime(ok), ok);
  for (const bad of ["25:00", "15:60", "15", "15:15:61", "ab:cd", "15:5"]) assert.ok(!validTime(bad), bad);
});

test("telegram ids", () => {
  assert.ok(validTelegram(""));
  assert.ok(validTelegram("12001200,78554211,-412554122"));
  assert.ok(!validTelegram("12001200,abc"));
  assert.ok(!validTelegram("123"));
});

test("profit lock needs both fields and the lock must be lower", () => {
  assert.equal(checkLimits(base), null);
  assert.ok(checkLimits({ ...base, lockAt: 1000 }));
  assert.ok(checkLimits({ ...base, lockMin: 500 }));
  assert.ok(checkLimits({ ...base, lockAt: 1000, lockMin: 1000 }));
  assert.equal(checkLimits({ ...base, lockAt: 1000, lockMin: 500 }), null);
});

test("trailing keeps a gap", () => {
  assert.ok(checkLimits({ ...base, trailEvery: 1000 }));
  assert.ok(checkLimits({ ...base, trailBy: 500 }));
  assert.ok(checkLimits({ ...base, trailEvery: 1000, trailBy: 1500 }));
  assert.equal(checkLimits({ ...base, trailEvery: 1000, trailBy: 500 }), null);
});

test("dependent options need their value", () => {
  assert.ok(checkLimits({ ...base, qtyOnMaxLoss: true }));
  assert.equal(checkLimits({ ...base, qtyOnMaxLoss: true, maxLossPerTrade: 500 }), null);
  assert.ok(checkLimits({ ...base, authRequired: true }));
  assert.equal(checkLimits({ ...base, authRequired: true, telegramIds: "12345678" }), null);
  assert.ok(checkLimits({ ...base, maxProfit: 800, lockAt: 1000, lockMin: 500 }));
});

test("emails", () => {
  for (const ok of ["", "a@b.co", "name.surname+tag@mail.example.com"]) assert.ok(validEmail(ok), ok);
  for (const bad of ["a", "a@b", "@b.co", "a b@c.co", "a@@b.co"]) assert.ok(!validEmail(bad), bad);
});

test("square-off 00:00:00 means off, like blank", () => {
  assert.equal(normalizeSqOff(""), "");
  assert.equal(normalizeSqOff("00:00:00"), "");
  assert.equal(normalizeSqOff("0:00"), "");
  assert.equal(normalizeSqOff(" 15:15:00 "), "15:15:00");
  assert.equal(normalizeSqOff("00:05:00"), "00:05:00");
});

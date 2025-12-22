import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "@rainlanguage/sqlite-web";
import {
  decodeFloatHex,
  encodeFloatHex,
} from "../fixtures/float-utils.js";

describe("FLOAT_ZERO_HEX Database Function", () => {
  let db: SQLiteWasmDatabase;
  const canonicalZeroHex = encodeFloatHex("0");

  beforeEach(async () => {
    db = await createTestDatabase();
  });

  afterEach(async () => {
    await cleanupDatabase(db);
  });

  it("should be registered and return the canonical zero hex string", async () => {
    const result = await db.query("SELECT FLOAT_ZERO_HEX() as zero_hex");
    const data = JSON.parse(result.value || "[]");
    expect(data).toHaveLength(1);
    expect(data[0].zero_hex).toBe(canonicalZeroHex);

    const decimal = decodeFloatHex(data[0].zero_hex);
    expect(decimal).toBe("0");
  });

  it("should integrate with queries storing and aggregating zero floats", async () => {
    await db.query(`
      CREATE TABLE float_zero_usage (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        amount TEXT NOT NULL
      )
    `);

    await db.query(
      "INSERT INTO float_zero_usage (amount) VALUES (FLOAT_ZERO_HEX())",
    );
    await db.query(
      "INSERT INTO float_zero_usage (amount) VALUES (FLOAT_ZERO_HEX())",
    );

    const stored = await db.query(
      "SELECT amount FROM float_zero_usage ORDER BY id",
    );
    const storedData = JSON.parse(stored.value || "[]");
    expect(storedData).toHaveLength(2);
    for (const row of storedData) {
      expect(row.amount).toBe(canonicalZeroHex);
      expect(decodeFloatHex(row.amount)).toBe("0");
    }

    const sumResult = await db.query(
      "SELECT FLOAT_SUM(amount) as total FROM float_zero_usage",
    );
    const sumData = JSON.parse(sumResult.value || "[]");
    expect(sumData).toHaveLength(1);
    expect(sumData[0].total).toBe(canonicalZeroHex);
    expect(decodeFloatHex(sumData[0].total)).toBe("0");
  });

  it("should return identical canonical values across multiple invocations", async () => {
    const result = await db.query(
      "SELECT FLOAT_ZERO_HEX() as first, FLOAT_ZERO_HEX() as second",
    );
    const data = JSON.parse(result.value || "[]");
    expect(data).toHaveLength(1);

    const first = data[0]?.first;
    const second = data[0]?.second;
    expect(first).toBe(canonicalZeroHex);
    expect(second).toBe(canonicalZeroHex);
    expect(first).toBe(second);
    expect(decodeFloatHex(first)).toBe("0");
    expect(decodeFloatHex(second)).toBe("0");
  });

  it("should apply canonical zero when used as a default value", async () => {
    await db.query(`
      CREATE TABLE float_zero_defaults (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        amount TEXT NOT NULL DEFAULT (FLOAT_ZERO_HEX())
      )
    `);

    await db.query("INSERT INTO float_zero_defaults DEFAULT VALUES");
    await db.query(
      "INSERT INTO float_zero_defaults (amount) VALUES (FLOAT_ZERO_HEX())",
    );

    const stored = await db.query(
      "SELECT amount FROM float_zero_defaults ORDER BY id",
    );
    const storedData = JSON.parse(stored.value || "[]");
    expect(storedData).toHaveLength(2);

    for (const row of storedData) {
      expect(row.amount).toBe(canonicalZeroHex);
      expect(decodeFloatHex(row.amount)).toBe("0");
    }
  });
});

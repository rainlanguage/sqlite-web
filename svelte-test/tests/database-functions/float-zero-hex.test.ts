import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "sqlite-web";
import { Float } from "@rainlanguage/float";

function decodeFloatHex(hex: string): string {
  const floatRes = Float.fromHex(hex as `0x${string}`);
  if (floatRes.error) {
    throw new Error(`fromHex failed: ${String(floatRes.error)}`);
  }
  const fmtRes = floatRes.value.format();
  if (fmtRes.error) {
    throw new Error(`format failed: ${String(fmtRes.error)}`);
  }
  return fmtRes.value as string;
}

function encodeFloatHex(decimal: string): `0x${string}` {
  const parseRes = Float.parse(decimal);
  if (parseRes.error) {
    throw new Error(
      `Float.parse failed: ${String(parseRes.error.msg ?? parseRes.error)}`,
    );
  }
  return parseRes.value.asHex();
}

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
});

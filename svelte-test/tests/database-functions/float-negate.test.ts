import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
  PerformanceTracker,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "sqlite-web";
import {
  createFloatHexMap,
  decodeFloatHex,
  toMixedCase,
  withoutPrefix,
} from "../fixtures/float-utils";

const floatHex = createFloatHexMap({
  zero: "0",
  smallPositive: "0.000000000000000123",
  onePointFive: "1.5",
  twoPointTwoFive: "2.25",
  highPrecision: "300.123456789012345678",
} as const);

describe("FLOAT_NEGATE Database Function", () => {
  let db: SQLiteWasmDatabase;
  let perf: PerformanceTracker;

  beforeEach(async () => {
    db = await createTestDatabase();
    perf = new PerformanceTracker();

    await db.query(`
      CREATE TABLE float_negate_test (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        amount TEXT NOT NULL,
        note TEXT
      )
    `);
  });

  afterEach(async () => {
    await cleanupDatabase(db);
  });

  describe("Function Availability", () => {
    it("should have FLOAT_NEGATE function available", async () => {
      try {
        const pragmaResult = await db.query("PRAGMA function_list");
        const functions = JSON.parse(pragmaResult.value || "[]");
        const entry = functions.find((f: any) => f.name === "FLOAT_NEGATE");
        expect(entry).toBeDefined();
      } catch (e) {}

      const sampleHex = floatHex.onePointFive;
      const neg = await db.query(`SELECT FLOAT_NEGATE('${sampleHex}') as neg`);
      expect(neg).toBeDefined();
      expect(neg.value).toBeDefined();
      const row = JSON.parse(neg.value || "[]")[0];
      expect(typeof row.neg).toBe("string");
      expect(row.neg.startsWith("0x")).toBe(true);
      expect(row.neg.length).toBe(66);
    });
  });

  describe("Basic FLOAT_NEGATE Functionality", () => {
    const samples: string[] = [
      floatHex.smallPositive,
      floatHex.onePointFive,
      withoutPrefix(floatHex.twoPointTwoFive),
      floatHex.highPrecision,
      floatHex.zero,
    ];

    it("should produce negation that sums to zero with original", async () => {
      for (const hex of samples) {
        const result = await db.query(`
          SELECT FLOAT_SUM(amount) as total FROM (
            SELECT '${hex}' as amount
            UNION ALL
            SELECT FLOAT_NEGATE('${hex}') as amount
          )
        `);
        const data = JSON.parse(result.value || "[]");
        const total = data[0].total as string;
        const decimalTotal = total === "0" ? total : decodeFloatHex(total);
        expect(decimalTotal).toBe("0");
      }
    });

    it("should handle whitespace around input", async () => {
      const hex = floatHex.twoPointTwoFive;
      const wrapped = `  ${hex}  `;
      const result = await db.query(`
        SELECT FLOAT_SUM(amount) as total FROM (
          SELECT '${wrapped}' as amount
          UNION ALL
          SELECT FLOAT_NEGATE('${wrapped}') as amount
        )
      `);
      const data = JSON.parse(result.value || "[]");
      const total = data[0].total as string;
      const decimalTotal = total === "0" ? total : decodeFloatHex(total);
      expect(decimalTotal).toBe("0");
    });

    it("should accept mixed-case 0x prefix and characters", async () => {
      await db.query("DELETE FROM float_negate_test");
      const mixed = toMixedCase(floatHex.highPrecision);
      const result = await db.query(`
        SELECT FLOAT_SUM(amount) as total FROM (
          SELECT '${mixed}' as amount
          UNION ALL
          SELECT FLOAT_NEGATE('${mixed}') as amount
        )
      `);
      const data = JSON.parse(result.value || "[]");
      const total = data[0].total as string;
      const decimalTotal = total === "0" ? total : decodeFloatHex(total);
      expect(decimalTotal).toBe("0");
    });
  });

  describe("NULL and Error Handling", () => {
    it("should return NULL when input is NULL", async () => {
      const res = await db.query("SELECT FLOAT_NEGATE(NULL) as neg");
      const data = JSON.parse(res.value || "[]");
      expect(data[0].neg).toBeNull();
    });

    it("should reject uppercase 0X prefix", async () => {
      const bad = floatHex.twoPointTwoFive.replace("0x", "0X");
      const result = await db.query(`SELECT FLOAT_NEGATE('${bad}') as neg`);
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse Float hex");
    });

    it("should reject invalid hex strings", async () => {
      const bad = "not_hex";
      const result = await db.query(`SELECT FLOAT_NEGATE('${bad}') as neg`);
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse Float hex");
    });

    it("should reject empty string", async () => {
      const bad = "";
      const result = await db.query(`SELECT FLOAT_NEGATE('${bad}') as neg`);
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse Float hex");
    });
  });
});

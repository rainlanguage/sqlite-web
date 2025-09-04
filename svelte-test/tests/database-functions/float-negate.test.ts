import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
  PerformanceTracker,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "sqlite-web";

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

      const sampleHex =
        "0xffffffff00000000000000000000000000000000000000000000000000000001";
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
    const samples = [
      "0xffffffff00000000000000000000000000000000000000000000000000000001",
      "0xffffffff00000000000000000000000000000000000000000000000000000005",
      "ffffffff0000000000000000000000000000000000000000000000000000000f",
      "0xfffffffe00000000000000000000000000000000000000000000000000002729",
      "0x0000000000000000000000000000000000000000000000000000000000000000",
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
        expect(data[0].total).toBe("0");
      }
    });

    it("should handle whitespace around input", async () => {
      const hex =
        "0xffffffff00000000000000000000000000000000000000000000000000000005";
      const wrapped = `  ${hex}  `;
      const result = await db.query(`
        SELECT FLOAT_SUM(amount) as total FROM (
          SELECT '${wrapped}' as amount
          UNION ALL
          SELECT FLOAT_NEGATE('${wrapped}') as amount
        )
      `);
      const data = JSON.parse(result.value || "[]");
      expect(data[0].total).toBe("0");
    });

    it("should accept mixed-case 0x prefix and characters", async () => {
      await db.query("DELETE FROM float_negate_test");
      const mixed =
        "0xFfFfFfFf0000000000000000000000000000000000000000000000000000000F";
      const result = await db.query(`
        SELECT FLOAT_SUM(amount) as total FROM (
          SELECT '${mixed}' as amount
          UNION ALL
          SELECT FLOAT_NEGATE('${mixed}') as amount
        )
      `);
      const data = JSON.parse(result.value || "[]");
      expect(data[0].total).toBe("0");
    });
  });

  describe("NULL and Error Handling", () => {
    it("should return NULL when input is NULL", async () => {
      const res = await db.query("SELECT FLOAT_NEGATE(NULL) as neg");
      const data = JSON.parse(res.value || "[]");
      expect(data[0].neg).toBeNull();
    });

    it("should reject uppercase 0X prefix", async () => {
      const bad =
        "0XFFFFFFFF00000000000000000000000000000000000000000000000000000069";
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

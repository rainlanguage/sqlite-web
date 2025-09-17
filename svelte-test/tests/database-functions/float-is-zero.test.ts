import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
  PerformanceTracker,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "sqlite-web";
import { Float } from "@rainlanguage/float";

const ZERO_HEX = Float.parse("0").value?.asHex() as string;
const SMALL_POS_HEX = Float.parse("0.1").value?.asHex() as string;
const OTHER_POS_HEX = Float.parse("0.5").value?.asHex() as string;

interface ZeroRow {
  value: string;
  is_zero: number;
}

describe("FLOAT_IS_ZERO Database Function", () => {
  let db: SQLiteWasmDatabase;
  let perf: PerformanceTracker;

  beforeEach(async () => {
    db = await createTestDatabase();
    perf = new PerformanceTracker();

    await db.query(`
      CREATE TABLE float_is_zero_test (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        value TEXT
      )
    `);
  });

  afterEach(async () => {
    await cleanupDatabase(db);
  });

  describe("Function Availability", () => {
    it("should have FLOAT_IS_ZERO function available", async () => {
      try {
        const pragmaResult = await db.query("PRAGMA function_list");
        expect(pragmaResult).toBeDefined();
        const functions = JSON.parse(pragmaResult.value || "[]");
        const isZeroEntry = functions.find(
          (f: any) => f.name === "FLOAT_IS_ZERO",
        );
        expect(isZeroEntry).toBeDefined();
      } catch (error) {}

      const result = await db.query(
        `SELECT FLOAT_IS_ZERO('${ZERO_HEX}') as is_zero`,
      );
      const data = JSON.parse(result.value || "[]");
      expect(Array.isArray(data)).toBe(true);
      expect(data[0].is_zero).toBe(1);
    });
  });

  describe("Zero Detection", () => {
    beforeEach(async () => {
      await db.query(`
        INSERT INTO float_is_zero_test (value) VALUES
        ('${ZERO_HEX}'),
        ('${SMALL_POS_HEX}'),
        ('${ZERO_HEX}'),
        ('  ${ZERO_HEX}  ')
      `);
    });

    it("should flag exact zero hex strings", async () => {
      const result = await db.query(
        "SELECT value, FLOAT_IS_ZERO(value) as is_zero FROM float_is_zero_test ORDER BY id",
      );
      const rows = JSON.parse(result.value || "[]") as ZeroRow[];
      expect(rows).toHaveLength(4);
      expect(rows[0].is_zero).toBe(1);
      expect(rows[1].is_zero).toBe(0);
      expect(rows[2].is_zero).toBe(1);
      expect(rows[3].is_zero).toBe(1);
    });

    it("should support filtering queries", async () => {
      const result = await db.query(`
        SELECT COUNT(*) as zero_count FROM float_is_zero_test
        WHERE FLOAT_IS_ZERO(value) = 1
      `);
      const data = JSON.parse(result.value || "[]");
      expect(data[0].zero_count).toBe(3);
    });
  });

  describe("Interactions With Other Functions", () => {
    it("should detect zero after negating equal values", async () => {
      perf.start("float-is-zero-balanced");
      const result = await db.query(`
        SELECT FLOAT_IS_ZERO(total) as balanced
        FROM (
          SELECT FLOAT_SUM(amount) as total FROM (
            SELECT '${SMALL_POS_HEX}' as amount
            UNION ALL
            SELECT FLOAT_NEGATE('${SMALL_POS_HEX}') as amount
          )
        )
      `);
      perf.end("float-is-zero-balanced");

      const data = JSON.parse(result.value || "[]");
      expect(data[0].balanced).toBe(1);
      expect(perf.getDuration("float-is-zero-balanced")).toBeLessThan(1000);
    });

    it("should return 0 when aggregate result is non-zero", async () => {
      const result = await db.query(`
        SELECT FLOAT_IS_ZERO(total) as balanced
        FROM (
          SELECT FLOAT_SUM(amount) as total FROM (
            SELECT '${SMALL_POS_HEX}' as amount
            UNION ALL
            SELECT '${OTHER_POS_HEX}' as amount
          )
        )
      `);
      const data = JSON.parse(result.value || "[]");
      expect(data[0].balanced).toBe(0);
    });
  });

  describe("NULL and Error Handling", () => {
    it("should return NULL when input is NULL", async () => {
      const result = await db.query("SELECT FLOAT_IS_ZERO(NULL) as flag");
      const data = JSON.parse(result.value || "[]");
      expect(data[0].flag).toBeNull();
    });

    it("should reject empty string inputs", async () => {
      const result = await db.query("SELECT FLOAT_IS_ZERO('') as flag");
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain(
        "Empty string is not a valid hex number",
      );
    });

    it("should reject invalid hex strings", async () => {
      const result = await db.query("SELECT FLOAT_IS_ZERO('not_hex') as flag");
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse Float hex");
    });
  });
});

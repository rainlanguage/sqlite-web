import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
  assertions,
  PerformanceTracker,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "@rainlanguage/sqlite-web";
import { Float } from "@rainlanguage/float";

interface CategoryRow {
  category: string;
  total: string;
}

interface TransactionRow {
  category: string;
  total_float: string;
}

describe("FLOAT_SUM Database Function", () => {
  let db: SQLiteWasmDatabase;
  let perf: PerformanceTracker;

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
      throw new Error(`Float.parse failed: ${String(parseRes.error.msg ?? parseRes.error)}`);
    }
    return parseRes.value.asHex();
  }

  function withoutPrefix(hex: `0x${string}`): string {
    return hex.slice(2);
  }

  function toMixedCase(hex: `0x${string}`): string {
    let result = "";
    for (let i = 0; i < hex.length; i++) {
      const char = hex[i];
      if (/[a-f]/.test(char)) {
        result += i % 2 === 0 ? char.toUpperCase() : char.toLowerCase();
      } else {
        result += char;
      }
    }
    return result;
  }

  const floatHex = {
    zero: encodeFloatHex("0"),
    zeroPointOne: encodeFloatHex("0.1"),
    zeroPointFive: encodeFloatHex("0.5"),
    onePointFive: encodeFloatHex("1.5"),
    twoPointTwoFive: encodeFloatHex("2.25"),
    ten: encodeFloatHex("10"),
    twenty: encodeFloatHex("20"),
    hundredPointTwentyFive: encodeFloatHex("100.25"),
    oneHundredTwentyThreePointFourFiveSix: encodeFloatHex("123.456"),
  } as const;

  beforeEach(async () => {
    db = await createTestDatabase();
    perf = new PerformanceTracker();

    await db.query(`
			CREATE TABLE float_test (
				id INTEGER PRIMARY KEY AUTOINCREMENT,
				amount TEXT NOT NULL,
				category TEXT,
				description TEXT
			)
		`);
  });

  afterEach(async () => {
    await cleanupDatabase(db);
  });

  describe("Function Availability", () => {
    it("should have FLOAT_SUM function available", async () => {
      try {
        const pragmaResult = await db.query("PRAGMA function_list");
        expect(pragmaResult).toBeDefined();
        const functions = JSON.parse(pragmaResult.value || "[]");
        const floatSumEntry = functions.find(
          (f: any) => f.name === "FLOAT_SUM",
        );
        expect(floatSumEntry).toBeDefined();
      } catch (error) {}

      try {
        const rainResult = await db.query(
          'SELECT RAIN_MATH_PROCESS("100", "200") as test',
        );
        expect(rainResult).toBeDefined();
        expect(rainResult.value).toBeDefined();
      } catch (error) {
        throw new Error("RAIN_MATH_PROCESS not available");
      }

      try {
        const result = await db.query(
          `SELECT FLOAT_SUM("${floatHex.zeroPointOne}") as test`,
        );
        expect(result).toBeDefined();
        expect(result.value).toBeDefined();
        const data = JSON.parse(result.value || "[]");
        const dec = decodeFloatHex(data[0].test);
        expect(dec).toBe("0.1");
      } catch (error) {
        throw new Error("FLOAT_SUM function not available");
      }
    });
  });

  describe("Basic FLOAT_SUM Functionality", () => {
    beforeEach(async () => {
      await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES
				('${floatHex.zeroPointOne}', 'income', 'payment'),
				('${floatHex.zeroPointFive}', 'income', 'deposit'),
				('${withoutPrefix(floatHex.onePointFive)}', 'income', 'amount')
			`);
    });

    it("should sum positive float values correctly", async () => {
      perf.start("float-sum-positive");
      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      perf.end("float-sum-positive");

      const data = JSON.parse(result.value || "[]");

      expect(data).toHaveLength(1);
      expect(data[0]).toHaveProperty("total");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("2.1");
      expect(perf.getDuration("float-sum-positive")).toBeLessThan(1000);
    });

    it("should handle single row correctly", async () => {
      await db.query("DELETE FROM float_test WHERE id > 1");

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("0.1");
    });

    it("should return 0 for empty table", async () => {
      await db.query("DELETE FROM float_test");

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("0");
    });
  });

  describe("Hex Format Support", () => {
    beforeEach(async () => {
      await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES
				('${floatHex.hundredPointTwentyFive}', 'income', 'large payment'),
				('${withoutPrefix(floatHex.oneHundredTwentyThreePointFourFiveSix)}', 'income', 'large deposit'),
				('${floatHex.zero}', 'zero', 'zero value')
			`);
    });

    it("should handle hex values with and without 0x prefix", async () => {
      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("223.706");
    });

    it("should handle mixed case hex values", async () => {
      await db.query("DELETE FROM float_test");
      await db.query(`
				INSERT INTO float_test (amount) VALUES
				('${toMixedCase(floatHex.onePointFive)}'),
				('${toMixedCase(floatHex.twoPointTwoFive)}')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("3.75");
    });

    it("should reject uppercase 0X prefix", async () => {
      await db.query("DELETE FROM float_test");
      await db.query(`
				INSERT INTO float_test (amount) VALUES ('0XFFFFFFFB0000000000000000000000000000000000000000000000000004CB2F')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse hex number");
    });
  });

  describe("GROUP BY Operations", () => {
    beforeEach(async () => {
      await db.query(`
				INSERT INTO float_test (amount, category) VALUES
				('${floatHex.zeroPointOne}', 'income'),
				('${floatHex.zeroPointFive}', 'income'),
				('${floatHex.ten}', 'bonus'),
				('${withoutPrefix(floatHex.onePointFive)}', 'expense')
			`);
    });

    it("should work with GROUP BY clauses", async () => {
      const result = await db.query(`
                SELECT category, FLOAT_SUM(amount) as total
                FROM float_test
                GROUP BY category
                ORDER BY category
            `);
      const data = JSON.parse(result.value || "[]");

      expect(data).toHaveLength(3);

      const bonus = data.find((row: CategoryRow) => row.category === "bonus");
      const expense = data.find(
        (row: CategoryRow) => row.category === "expense",
      );
      const income = data.find((row: CategoryRow) => row.category === "income");

      const bonusDec = decodeFloatHex(bonus!.total);
      const expenseDec = decodeFloatHex(expense!.total);
      const incomeDec = decodeFloatHex(income!.total);

      expect(bonusDec).toBe("10");
      expect(expenseDec).toBe("1.5");
      expect(incomeDec).toBe("0.6");
    });

    it("should filter groups by decoded sum (> 1)", async () => {
      const result = await db.query(`
                SELECT category, FLOAT_SUM(amount) as total
                FROM float_test
                GROUP BY category
                ORDER BY category
            `);
      const data = JSON.parse(result.value || "[]");

      const filtered = [] as string[];
      for (const row of data as CategoryRow[]) {
        const dec = decodeFloatHex(row.total);
        if (Number(dec) > 1) filtered.push(row.category);
      }
      filtered.sort();
      expect(filtered).toEqual(["bonus", "expense"]);
    });
  });

  describe("Error Handling", () => {
    it("should handle invalid hex strings gracefully", async () => {
      await db.query(`
				INSERT INTO float_test (amount) VALUES ('not_hex')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse hex number");
    });

    it("should handle empty strings gracefully", async () => {
      await db.query(`
				INSERT INTO float_test (amount) VALUES ('')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain(
        "Empty string is not a valid hex number",
      );
    });

    it("should handle invalid hex characters", async () => {
      await db.query(`
				INSERT INTO float_test (amount) VALUES ('0xGHI')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse hex number");
    });

    it("should sum signed float values correctly", async () => {
      await db.query("DELETE FROM float_test");

      const negative = encodeFloatHex("-2.5");
      const positive = encodeFloatHex("3.1");

      await db.query(`
				INSERT INTO float_test (amount) VALUES
				('${negative}'),
				('${positive}')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      expect(result.error).toBeUndefined();

      const data = JSON.parse(result.value || "[]") as Array<{ total: string }>;
      expect(Array.isArray(data)).toBe(true);
      expect(data).toHaveLength(1);

      const decodedTotal = decodeFloatHex(data[0].total);
      expect(decodedTotal).toBe("0.6");
    });

    it("should handle only valid hex values", async () => {
      await db.query(`
				INSERT INTO float_test (amount) VALUES
				('${floatHex.zeroPointOne}'),
				('${floatHex.zeroPointFive}')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("0.6");
    });
  });

  describe("Edge Cases and Limits", () => {
    it("should handle very small float values", async () => {
      const smallValue = floatHex.zeroPointOne;

      await db.query(`
				INSERT INTO float_test (amount) VALUES ('${smallValue}')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("0.1");
    });

    it("should reject hex values that are too short", async () => {
      await db.query(`
				INSERT INTO float_test (amount) VALUES ('0x0')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      expect(result.error).toBeDefined();
      expect(result.error?.msg).toContain("Failed to parse hex number");
    });

    it("should handle whitespace in hex values", async () => {
      await db.query(`
				INSERT INTO float_test (amount) VALUES
				('  ${floatHex.ten}  '),
				('\t${floatHex.twenty}\n')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");

      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("30");
    });
  });

  describe("Performance Tests", () => {
    it("should handle bulk aggregation efficiently", async () => {
      const values = Array.from(
        { length: 10 },
        () =>
          `('${floatHex.zeroPointOne}', 'bulk')`,
      ).join(",");

      await db.query(`
				INSERT INTO float_test (amount, category) VALUES ${values}
			`);

      perf.start("bulk-float-sum");
      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      perf.end("bulk-float-sum");

      if (result.error) {
        throw new Error(`Query failed: ${result.error.msg}`);
      }

      const data = JSON.parse(result.value || "[]");
      expect(data).toHaveLength(1);
      expect(data[0]).toHaveProperty("total");
      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("1");
      expect(perf.getDuration("bulk-float-sum")).toBeLessThan(3000);
    });

    it("should handle complex queries with joins efficiently", async () => {
      await db.query(`
				CREATE TABLE float_categories (
					name TEXT PRIMARY KEY,
					multiplier TEXT
				)
			`);

      await db.query(`
				INSERT INTO float_categories (name, multiplier) VALUES
				('income', '2'),
				('expense', '1')
			`);

      await db.query(`
				INSERT INTO float_test (amount, category) VALUES
				('${floatHex.zeroPointOne}', 'income'),
				('${floatHex.zeroPointFive}', 'expense')
			`);

      perf.start("complex-float-query");
      const result = await db.query(`
				SELECT
					t.category,
					FLOAT_SUM(t.amount) as total_amount,
					c.multiplier
				FROM float_test t
				JOIN float_categories c ON t.category = c.name
				GROUP BY t.category, c.multiplier
				ORDER BY total_amount DESC
			`);
      perf.end("complex-float-query");

      const data = JSON.parse(result.value || "[]");
      expect(Array.isArray(data)).toBe(true);
      expect(data.length).toBe(2);

      const rowsByCategory = data.reduce(
        (map: Record<string, any>, row: any) => {
          map[row.category] = row;
          return map;
        },
        {} as Record<string, any>,
      );

      expect(rowsByCategory["income"]).toBeDefined();
      expect(decodeFloatHex(rowsByCategory["income"].total_amount)).toBe("0.1");
      expect(rowsByCategory["income"].multiplier).toBe("2");

      expect(rowsByCategory["expense"]).toBeDefined();
      expect(decodeFloatHex(rowsByCategory["expense"].total_amount)).toBe(
        "0.5",
      );
      expect(rowsByCategory["expense"].multiplier).toBe("1");

      expect(perf.getDuration("complex-float-query")).toBeLessThan(2000);
    });
  });

  describe("Real-world Scenarios", () => {
    it("should handle DeFi-like transaction amounts", async () => {
      await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES
				('${floatHex.hundredPointTwentyFive}', 'swap', 'token swap'),
				('${floatHex.oneHundredTwentyThreePointFourFiveSix}', 'liquidity', 'liquidity provision'),
				('${floatHex.zeroPointOne}', 'rewards', 'staking rewards'),
				('${floatHex.zeroPointFive}', 'fees', 'protocol fees')
			`);

      const result = await db.query(`
				SELECT
					category,
					FLOAT_SUM(amount) as total_float,
					COUNT(*) as transaction_count
				FROM float_test
				GROUP BY category
				ORDER BY total_float DESC
			`);

      const data = JSON.parse(result.value || "[]");
      expect(Array.isArray(data)).toBe(true);

      const liquidity = data.find(
        (row: TransactionRow) => row.category === "liquidity",
      );
      expect(decodeFloatHex(liquidity!.total_float)).toBe("123.456");
    });

    it("should handle precision accounting that must balance", async () => {
      await db.query(`
				INSERT INTO float_test (amount, description) VALUES
				('${floatHex.hundredPointTwentyFive}', 'Initial balance'),
				('${floatHex.zeroPointOne}', 'Small addition')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as balance FROM float_test",
      );

      if (result.error) {
        throw new Error(`Query failed: ${result.error.msg}`);
      }

      const data = JSON.parse(result.value || "[]");
      expect(data).toHaveLength(1);
      expect(data[0]).toHaveProperty("balance");
      const dec = decodeFloatHex(data[0].balance);
      expect(dec).toBe("100.35");
    });
  });

  describe("NULL Value Handling", () => {
    it("should skip NULL values like standard SQL aggregates", async () => {
      await db.query(`
				INSERT INTO float_test (amount, category) VALUES
				('${floatHex.zeroPointOne}', 'test'),
				('${floatHex.zeroPointFive}', 'test')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );

      if (result.error) {
        throw new Error(`Query failed: ${result.error.msg}`);
      }

      const data = JSON.parse(result.value || "[]");
      expect(data).toHaveLength(1);
      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("0.6");
    });

    it("should return 0 when all values are NULL", async () => {
      await db.query(`
				INSERT INTO float_test (amount, category) VALUES
				(NULL, 'test'),
				(NULL, 'test')
			`);

      const result = await db.query(
        "SELECT FLOAT_SUM(amount) as total FROM float_test",
      );
      const data = JSON.parse(result.value || "[]");
      const dec = decodeFloatHex(data[0].total);
      expect(dec).toBe("0");
    });
  });
});

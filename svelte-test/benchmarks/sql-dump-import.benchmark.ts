import { describe, expect, it } from "vitest";
import { createTestDatabase } from "../tests/fixtures/test-helpers.js";

// Manual comparison of equal rows using the old statement-array path and the
// bounded grouped-SQL path. Run alone to avoid concurrent OPFS/CPU contention.
describe("SQL dump import benchmark", () => {
  it("records sequential equal-data timings", async () => {
    const db = await createTestDatabase(`sql-import-benchmark-${Date.now()}`);
    const rowCount = 10_000;
    await db.query("CREATE TABLE old_import (id INTEGER PRIMARY KEY, value TEXT)");
    await db.query("CREATE TABLE new_import (id INTEGER PRIMARY KEY, value TEXT)");

    const oldStart = performance.now();
    const statements = Array.from({ length: rowCount }, (_, i) => ({
      sql: `INSERT INTO old_import VALUES (${i}, 'value ${i}')`,
    }));
    const oldResult = await db.transaction(statements);
    const oldMs = performance.now() - oldStart;
    expect(oldResult.error).toBeUndefined();

    const newStart = performance.now();
    const id = (await db.beginSqlDumpImport()).value!;
    for (let offset = 0; offset < rowCount; offset += 256) {
      const tuples = Array.from(
        { length: Math.min(256, rowCount - offset) },
        (_, i) => `(${offset + i}, 'value ${offset + i}')`,
      );
      const sql = `INSERT INTO new_import VALUES ${tuples.join(",")};\n`;
      const result = await db.appendSqlDumpChunk(id, sql);
      expect(result.error).toBeUndefined();
    }
    const finish = await db.finishSqlDumpImport(id);
    const newMs = performance.now() - newStart;
    expect(finish.error).toBeUndefined();

    const oldData = await db.query(
      "SELECT COUNT(*) AS count, SUM(id) AS ids, SUM(LENGTH(value)) AS bytes FROM old_import",
    );
    const newData = await db.query(
      "SELECT COUNT(*) AS count, SUM(id) AS ids, SUM(LENGTH(value)) AS bytes FROM new_import",
    );
    expect(JSON.parse(oldData.value || "[]")).toEqual(JSON.parse(newData.value || "[]"));
    expect(JSON.parse(newData.value || "[]")[0].count).toBe(rowCount);
    console.info(`SQL import benchmark: ${rowCount} rows; statement array ${oldMs.toFixed(1)} ms; grouped chunks ${newMs.toFixed(1)} ms`);
  }, 120_000);
});

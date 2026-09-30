import { describe, expect, it } from "vitest";
import { createTestDatabase } from "../tests/fixtures/test-helpers.js";

// Run alone to avoid concurrent OPFS/CPU contention. The two grouped cases use
// identical SQL so their difference isolates the API rather than row grouping.
// This fixed-order, single-run comparison uses warm storage and small chunks;
// treat it as a smoke measurement, not a production speedup estimate.
describe("SQL dump import benchmark", () => {
  it("records sequential equal-data timings", async () => {
    const db = await createTestDatabase(`sql-import-benchmark-${Date.now()}`);
    const rowCount = 10_000;
    expect(
      (await db.query("CREATE TABLE benchmark_import (id INTEGER PRIMARY KEY, value TEXT)")).error,
    ).toBeUndefined();

    const singleStart = performance.now();
    const statements = Array.from({ length: rowCount }, (_, i) => ({
      sql: `INSERT INTO benchmark_import VALUES (${i}, 'value ${i}')`,
    }));
    const singleResult = await db.transaction(statements);
    const singleMs = performance.now() - singleStart;
    expect(singleResult.error).toBeUndefined();

    const readData = async () => {
      const result = await db.query(
        "SELECT COUNT(*) AS count, SUM(id) AS ids, SUM(LENGTH(value)) AS bytes FROM benchmark_import",
      );
      expect(result.error).toBeUndefined();
      return JSON.parse(result.value || "[]");
    };
    const reference = await readData();
    expect(reference[0].count).toBe(rowCount);
    expect((await db.query("DELETE FROM benchmark_import")).error).toBeUndefined();

    // Build once outside both grouped timings; each timed path receives the same
    // prebuilt strings. The single-row timing above includes its array creation.
    const groupedSql: string[] = [];
    for (let offset = 0; offset < rowCount; offset += 256) {
      const tuples = Array.from(
        { length: Math.min(256, rowCount - offset) },
        (_, i) => `(${offset + i}, 'value ${offset + i}')`,
      );
      groupedSql.push(`INSERT INTO benchmark_import VALUES ${tuples.join(",")};\n`);
    }

    const groupedStart = performance.now();
    const groupedResult = await db.transaction(groupedSql.map((sql) => ({ sql })));
    const groupedMs = performance.now() - groupedStart;
    expect(groupedResult.error).toBeUndefined();
    expect(await readData()).toEqual(reference);
    expect((await db.query("DELETE FROM benchmark_import")).error).toBeUndefined();

    const chunkStart = performance.now();
    const begin = await db.beginSqlDumpImport();
    expect(begin.error).toBeUndefined();
    const id = begin.value!;
    for (const sql of groupedSql) {
      expect((await db.appendSqlDumpChunk(id, sql)).error).toBeUndefined();
    }
    const finish = await db.finishSqlDumpImport(id);
    const chunkMs = performance.now() - chunkStart;
    expect(finish.error).toBeUndefined();
    expect(await readData()).toEqual(reference);

    console.info(
      `SQL import benchmark: ${rowCount} rows; single-row transaction (including construction) ${singleMs.toFixed(1)} ms; grouped transaction ${groupedMs.toFixed(1)} ms; identical grouped chunks ${chunkMs.toFixed(1)} ms`,
    );
  }, 120_000);
});

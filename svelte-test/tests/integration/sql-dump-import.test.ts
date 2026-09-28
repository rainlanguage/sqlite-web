import { describe, expect, it } from "vitest";
import { createTestDatabase } from "../fixtures/test-helpers.js";

describe("SQL dump import", () => {
  const dbName = `sql-dump-import-${Date.now()}`;

  it("keeps a multi-chunk import private across clients until commit", async () => {
    const leader = await createTestDatabase(dbName);
    const follower = await createTestDatabase(dbName);
    await leader.query("DROP TABLE IF EXISTS streamed_dump_test");
    await leader.query("CREATE TABLE streamed_dump_test (id INTEGER PRIMARY KEY, value TEXT)");

    const begin = await follower.beginSqlDumpImport();
    expect(begin.error).toBeUndefined();
    const id = begin.value!;
    const first = await follower.appendSqlDumpChunk(
      id,
      "BEGIN; INSERT INTO streamed_dump_test VALUES (1, 'one; value'); INS",
    );
    expect(first.error).toBeUndefined();

    const competingRead = await leader.query("SELECT COUNT(*) AS count FROM streamed_dump_test");
    expect(competingRead.error?.readableMsg ?? competingRead.error?.msg ?? "").toContain(
      "SQL dump import is in progress",
    );

    const second = await follower.appendSqlDumpChunk(
      id,
      "ERT INTO streamed_dump_test VALUES (2, 'two'); COMMIT;",
    );
    expect(second.error).toBeUndefined();
    const finish = await follower.finishSqlDumpImport(id);
    expect(finish.error).toBeUndefined();
    expect(finish.value).toContain("Imported 2 SQL statements");

    const finalRead = await leader.query("SELECT value FROM streamed_dump_test ORDER BY id");
    expect(JSON.parse(finalRead.value || "[]").map((row: { value: string }) => row.value)).toEqual([
      "one; value",
      "two",
    ]);
    await leader.query("DROP TABLE streamed_dump_test");
  });

  it("rolls back a malformed chunk", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS failed_dump_test");
    await db.query("CREATE TABLE failed_dump_test (id INTEGER PRIMARY KEY)");

    const id = (await db.beginSqlDumpImport()).value!;
    const failed = await db.appendSqlDumpChunk(
      id,
      "INSERT INTO failed_dump_test VALUES (1); INSERT INTO failed_dump_test (missing) VALUES (2);",
    );
    expect(failed.error).toBeDefined();
    const result = await db.query("SELECT COUNT(*) AS count FROM failed_dump_test");
    expect(JSON.parse(result.value || "[]")[0].count).toBe(0);
    await db.query("DROP TABLE failed_dump_test");
  });

  it("requires the caller to await each chunk", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS serial_dump_test");
    await db.query("CREATE TABLE serial_dump_test (id INTEGER PRIMARY KEY)");
    const id = (await db.beginSqlDumpImport()).value!;

    const results = await Promise.all([
      db.appendSqlDumpChunk(id, "INSERT INTO serial_dump_test VALUES (1);"),
      db.appendSqlDumpChunk(id, "INSERT INTO serial_dump_test VALUES (2);"),
    ]);
    expect(results.filter((result) => result.error).length).toBe(1);
    expect((await db.finishSqlDumpImport(id)).error).toBeUndefined();
    const count = await db.query("SELECT COUNT(*) AS count FROM serial_dump_test");
    expect(JSON.parse(count.value || "[]")[0].count).toBe(1);
    await db.query("DROP TABLE serial_dump_test");
  });

  it("rejects oversized UTF-8 chunks before dispatch and cancels the import", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS unicode_dump_test");
    await db.query("CREATE TABLE unicode_dump_test (id INTEGER PRIMARY KEY)");
    const id = (await db.beginSqlDumpImport()).value!;
    expect((await db.appendSqlDumpChunk(id, "INSERT INTO unicode_dump_test VALUES (1);")).error).toBeUndefined();

    const oversized = await db.appendSqlDumpChunk(id, "界".repeat(200_000));
    expect(oversized.error).toBeDefined();
    const count = await db.query("SELECT COUNT(*) AS count FROM unicode_dump_test");
    expect(JSON.parse(count.value || "[]")[0].count).toBe(0);
    await db.query("DROP TABLE unicode_dump_test");
  });

  it("does not race an invalid chunk's cancellation with an active append", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS concurrent_invalid_dump_test");
    await db.query("CREATE TABLE concurrent_invalid_dump_test (id INTEGER PRIMARY KEY)");
    const id = (await db.beginSqlDumpImport()).value!;
    const first = db.appendSqlDumpChunk(
      id,
      `INSERT INTO concurrent_invalid_dump_test VALUES (1); /*${"x".repeat(400_000)}*/`,
    );
    const invalid = db.appendSqlDumpChunk(id, "界".repeat(200_000));
    const [firstResult, invalidResult] = await Promise.all([first, invalid]);
    expect(firstResult.error).toBeUndefined();
    expect(invalidResult.error?.readableMsg ?? invalidResult.error?.msg ?? "").toContain(
      "Wait for the previous SQL dump import request",
    );

    expect((await db.cancelSqlDumpImport(id)).error).toBeUndefined();
    const count = await db.query("SELECT COUNT(*) AS count FROM concurrent_invalid_dump_test");
    expect(JSON.parse(count.value || "[]")[0].count).toBe(0);
    await db.query("DROP TABLE concurrent_invalid_dump_test");
  });
});

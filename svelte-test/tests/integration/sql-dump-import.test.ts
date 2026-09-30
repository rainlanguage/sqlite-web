import { afterEach, describe, expect, it, vi } from "vitest";
import type { SQLiteWasmDatabase } from "@rainlanguage/sqlite-web";
import { createTestDatabase } from "../fixtures/test-helpers.js";

describe("SQL dump import", () => {
  const dbName = `sql-dump-import-${Date.now()}`;
  const cancellations: Array<() => Promise<unknown>> = [];

  async function beginImport(db: SQLiteWasmDatabase): Promise<string> {
    const begin = await db.beginSqlDumpImport();
    expect(begin.error).toBeUndefined();
    const id = begin.value!;
    cancellations.push(() => db.cancelSqlDumpImport(id));
    return id;
  }

  afterEach(async () => {
    try {
      // Cancel even when an assertion fails. Already finished/failed sessions
      // return an error without disturbing a later import.
      for (const cancel of cancellations.splice(0)) await cancel();
    } finally {
      vi.restoreAllMocks();
    }
  });

  it("keeps a multi-chunk import private across clients until commit", async () => {
    const leader = await createTestDatabase(dbName);
    const follower = await createTestDatabase(dbName);
    await leader.query("DROP TABLE IF EXISTS streamed_dump_test");
    await leader.query(
      "CREATE TABLE streamed_dump_test (id INTEGER PRIMARY KEY, value TEXT)",
    );

    const id = await beginImport(follower);
    const first = await follower.appendSqlDumpChunk(
      id,
      "BEGIN; INSERT INTO streamed_dump_test VALUES (1, 'one; value'); INS",
    );
    expect(first.error).toBeUndefined();

    const competingRead = await leader.query(
      "SELECT COUNT(*) AS count FROM streamed_dump_test",
    );
    expect(
      competingRead.error?.readableMsg ?? competingRead.error?.msg ?? "",
    ).toContain("SQL dump import is in progress");

    const second = await follower.appendSqlDumpChunk(
      id,
      "ERT INTO streamed_dump_test VALUES (2, 'two'); COMMIT;",
    );
    expect(second.error).toBeUndefined();
    const finish = await follower.finishSqlDumpImport(id);
    expect(finish.error).toBeUndefined();
    expect(finish.value).toContain("Imported 2 SQL statements");

    const finalRead = await leader.query(
      "SELECT value FROM streamed_dump_test ORDER BY id",
    );
    expect(
      JSON.parse(finalRead.value || "[]").map(
        (row: { value: string }) => row.value,
      ),
    ).toEqual(["one; value", "two"]);
    await leader.query("DROP TABLE streamed_dump_test");
  });

  it("rolls back a malformed chunk", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS failed_dump_test");
    await db.query("CREATE TABLE failed_dump_test (id INTEGER PRIMARY KEY)");

    const id = await beginImport(db);
    const failed = await db.appendSqlDumpChunk(
      id,
      "INSERT INTO failed_dump_test VALUES (1); INSERT INTO failed_dump_test (missing) VALUES (2);",
    );
    expect(failed.error).toBeDefined();
    const result = await db.query(
      "SELECT COUNT(*) AS count FROM failed_dump_test",
    );
    expect(JSON.parse(result.value || "[]")[0].count).toBe(0);
    await db.query("DROP TABLE failed_dump_test");
  });

  it("requires the caller to await each chunk", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS serial_dump_test");
    await db.query("CREATE TABLE serial_dump_test (id INTEGER PRIMARY KEY)");
    const id = await beginImport(db);

    const results = await Promise.all([
      db.appendSqlDumpChunk(id, "INSERT INTO serial_dump_test VALUES (1);"),
      db.appendSqlDumpChunk(id, "INSERT INTO serial_dump_test VALUES (2);"),
    ]);
    expect(results.filter((result) => result.error).length).toBe(1);
    expect((await db.finishSqlDumpImport(id)).error).toBeUndefined();
    const count = await db.query(
      "SELECT COUNT(*) AS count FROM serial_dump_test",
    );
    expect(JSON.parse(count.value || "[]")[0].count).toBe(1);
    await db.query("DROP TABLE serial_dump_test");
  });

  it("rejects oversized UTF-8 chunks before dispatch and cancels the import", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS unicode_dump_test");
    await db.query("CREATE TABLE unicode_dump_test (id INTEGER PRIMARY KEY)");
    const id = await beginImport(db);
    expect(
      (
        await db.appendSqlDumpChunk(
          id,
          "INSERT INTO unicode_dump_test VALUES (1);",
        )
      ).error,
    ).toBeUndefined();

    const dispatch = vi.spyOn(Worker.prototype, "postMessage");
    const oversized = await db.appendSqlDumpChunk(id, "界".repeat(200_000));
    expect(oversized.error).toBeDefined();
    expect(
      dispatch.mock.calls.filter(
        ([message]) => message?.action?.kind === "chunk",
      ),
    ).toHaveLength(0);
    const count = await db.query(
      "SELECT COUNT(*) AS count FROM unicode_dump_test",
    );
    expect(JSON.parse(count.value || "[]")[0].count).toBe(0);
    await db.query("DROP TABLE unicode_dump_test");
  });

  it("does not race an invalid chunk's cancellation with an active append", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("DROP TABLE IF EXISTS concurrent_invalid_dump_test");
    await db.query(
      "CREATE TABLE concurrent_invalid_dump_test (id INTEGER PRIMARY KEY)",
    );
    const id = await beginImport(db);
    const first = db.appendSqlDumpChunk(
      id,
      `INSERT INTO concurrent_invalid_dump_test VALUES (1); /*${"x".repeat(400_000)}*/`,
    );
    const invalid = db.appendSqlDumpChunk(id, "界".repeat(200_000));
    const [firstResult, invalidResult] = await Promise.all([first, invalid]);
    expect(firstResult.error).toBeUndefined();
    expect(
      invalidResult.error?.readableMsg ?? invalidResult.error?.msg ?? "",
    ).toContain("Wait for the previous SQL dump import request");

    expect((await db.cancelSqlDumpImport(id)).error).toBeUndefined();
    const count = await db.query(
      "SELECT COUNT(*) AS count FROM concurrent_invalid_dump_test",
    );
    expect(JSON.parse(count.value || "[]")[0].count).toBe(0);
    await db.query("DROP TABLE concurrent_invalid_dump_test");
  });

  it.each(["\uD83D", "\uDE00"])(
    "rejects an unpaired surrogate before dispatch and rolls back",
    async (surrogate) => {
      const db = await createTestDatabase(dbName);
      await db.query(
        "CREATE TABLE IF NOT EXISTS surrogate_dump_test (value TEXT)",
      );
      const id = await beginImport(db);
      expect(
        (
          await db.appendSqlDumpChunk(
            id,
            "INSERT INTO surrogate_dump_test VALUES ('before');",
          )
        ).error,
      ).toBeUndefined();
      const dispatch = vi.spyOn(Worker.prototype, "postMessage");
      const invalid = await db.appendSqlDumpChunk(
        id,
        `INSERT INTO surrogate_dump_test VALUES ('${surrogate}`,
      );
      expect(invalid.error?.readableMsg ?? invalid.error?.msg ?? "").toContain(
        "invalid UTF-16",
      );
      expect(
        dispatch.mock.calls.filter(
          ([message]) => message?.action?.kind === "chunk",
        ),
      ).toHaveLength(0);
      const rows = await db.query(
        "SELECT COUNT(*) AS count FROM surrogate_dump_test",
      );
      expect(rows.error).toBeUndefined();
      expect(JSON.parse(rows.value || "[]")[0].count).toBe(0);
      await db.query("DROP TABLE surrogate_dump_test");
    },
  );

  it("preserves complete surrogate pairs across SQL chunks", async () => {
    const db = await createTestDatabase(dbName);
    await db.query("CREATE TABLE emoji_dump_test (value TEXT)");
    const id = await beginImport(db);
    expect(
      (
        await db.appendSqlDumpChunk(
          id,
          "INSERT INTO emoji_dump_test VALUES ('😀",
        )
      ).error,
    ).toBeUndefined();
    expect((await db.appendSqlDumpChunk(id, "😃');")).error).toBeUndefined();
    expect((await db.finishSqlDumpImport(id)).error).toBeUndefined();
    const rows = await db.query("SELECT value FROM emoji_dump_test");
    expect(rows.error).toBeUndefined();
    expect(JSON.parse(rows.value || "[]")[0].value).toBe("😀😃");
    await db.query("DROP TABLE emoji_dump_test");
  });
});

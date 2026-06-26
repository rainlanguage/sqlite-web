import { describe, it, expect } from "vitest";
import { createTestDatabase } from "../fixtures/test-helpers.js";

describe("Transaction interleaving", () => {
  // Reuse one OPFS database name in this file because sqlite-web does not expose
  // a close API; opening independent sahpool handles per test can conflict.
  const dbName = `transaction-interleaving-${Date.now()}`;

  it("executes a transaction batch and returns the last row result", async () => {
    const db = await createTestDatabase(dbName);

    await db.query("DROP TABLE IF EXISTS batch_api_test");
    const batchResult = await db.transaction([
      {
        sql: "CREATE TABLE batch_api_test (id INTEGER PRIMARY KEY, name TEXT)",
      },
      {
        sql: "SELECT 'first' as label",
      },
      {
        sql: "INSERT INTO batch_api_test (name) VALUES (?)",
        params: ["Alice"],
      },
      {
        sql: "SELECT COUNT(*) as count FROM batch_api_test",
      },
    ]);

    expect(batchResult.error).toBeUndefined();
    const batchRows = JSON.parse(batchResult.value || "[]");
    expect(batchRows[0].count).toBe(1);

    await db.query("DROP TABLE IF EXISTS batch_api_test");
  });

  it("rolls back all transaction statements after a mid-batch failure", async () => {
    const db = await createTestDatabase(dbName);

    await db.query("DROP TABLE IF EXISTS batch_api_rollback_test");
    await db.query(
      "CREATE TABLE batch_api_rollback_test (id INTEGER PRIMARY KEY, name TEXT)",
    );

    const rollbackResult = await db.transaction([
      {
        sql: "INSERT INTO batch_api_rollback_test (id, name) VALUES (1, 'Alice')",
      },
      {
        sql: "INSERT INTO missing_batch_api_table (id) VALUES (1)",
      },
    ]);

    expect(
      rollbackResult.error?.readableMsg || rollbackResult.error?.msg || "",
    ).toContain("Batch statement 2 failed");

    const countResult = await db.query(
      "SELECT COUNT(*) as count FROM batch_api_rollback_test",
    );
    const rollbackRows = JSON.parse(countResult.value || "[]");
    expect(rollbackRows[0].count).toBe(0);

    await db.query("DROP TABLE IF EXISTS batch_api_rollback_test");
  });

  it("does not expose partial transaction state to a competing client", async () => {
    const db = await createTestDatabase(dbName);
    const clientB = await createTestDatabase(dbName);

    await db.query("DROP TABLE IF EXISTS batch_api_visibility_test");
    await db.query(
      "CREATE TABLE batch_api_visibility_test (id INTEGER PRIMARY KEY)",
    );

    const batchVisibility = db.transaction([
      {
        sql: "INSERT INTO batch_api_visibility_test DEFAULT VALUES",
      },
      {
        sql: "INSERT INTO batch_api_visibility_test DEFAULT VALUES",
      },
    ]);
    const competingRead = clientB.query(
      "SELECT COUNT(*) as count FROM batch_api_visibility_test",
    );
    const [visibilityResult, competingReadResult] = await Promise.all([
      batchVisibility,
      competingRead,
    ]);

    expect(visibilityResult.error).toBeUndefined();
    expect(competingReadResult.error).toBeUndefined();

    const competingRows = JSON.parse(competingReadResult.value || "[]");
    expect([0, 2]).toContain(competingRows[0].count);

    const finalVisibilityResult = await db.query(
      "SELECT COUNT(*) as count FROM batch_api_visibility_test",
    );
    const finalVisibilityRows = JSON.parse(finalVisibilityResult.value || "[]");
    expect(finalVisibilityRows[0].count).toBe(2);

    await db.query("DROP TABLE IF EXISTS batch_api_visibility_test");
  });

  it("allows a follower client to originate a transaction batch", async () => {
    const db = await createTestDatabase(dbName);
    const clientB = await createTestDatabase(dbName);

    await db.query("DROP TABLE IF EXISTS batch_api_follower_test");
    await db.query(
      "CREATE TABLE batch_api_follower_test (id INTEGER PRIMARY KEY, name TEXT)",
    );

    const followerTransaction = await clientB.transaction([
      {
        sql: "INSERT INTO batch_api_follower_test (name) VALUES (?)",
        params: ["Bob"],
      },
      {
        sql: "INSERT INTO batch_api_follower_test (name) VALUES (?)",
        params: ["Carol"],
      },
    ]);

    expect(followerTransaction.error).toBeUndefined();

    const countResult = await db.query(
      "SELECT COUNT(*) as count FROM batch_api_follower_test",
    );
    const countRows = JSON.parse(countResult.value || "[]");
    expect(countRows[0].count).toBe(2);

    await db.query("DROP TABLE IF EXISTS batch_api_follower_test");
  });

  it("reproduces split transaction interleaving with separate query calls", async () => {
    const db = await createTestDatabase(dbName);
    const clientB = await createTestDatabase(dbName);

    await db.query("DROP TABLE IF EXISTS interleave_test");
    await db.query("CREATE TABLE interleave_test (id INTEGER PRIMARY KEY)");

    await db.query("BEGIN TRANSACTION");
    const clientBBegin = await clientB.query("BEGIN TRANSACTION");

    const message =
      clientBBegin.error?.readableMsg ?? clientBBegin.error?.msg ?? "";
    expect(message).toContain("cannot start a transaction within a transaction");

    await db.query("ROLLBACK");
    await db.query("DROP TABLE IF EXISTS interleave_test");
  });
});

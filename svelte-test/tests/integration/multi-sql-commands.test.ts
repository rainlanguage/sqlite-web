import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  createTestDatabase,
  cleanupDatabase,
  assertions,
} from "../fixtures/test-helpers.js";
import type { SQLiteWasmDatabase } from "sqlite-web";

describe("Multi-SQL Commands (UI Integration)", () => {
  let db: SQLiteWasmDatabase;

  beforeEach(async () => {
    db = await createTestDatabase();
  });

  afterEach(async () => {
    if (db) await cleanupDatabase(db);
  });

  it("executes multiple statements in a single query call", async () => {
    const sql = `
      CREATE TABLE multi_ui (id INTEGER PRIMARY KEY, name TEXT);
      INSERT INTO multi_ui (name) VALUES ('Alice');
      INSERT INTO multi_ui (name) VALUES ('Bob');
      SELECT COUNT(*) as count FROM multi_ui;
    `;

    const result = await db.query(sql);
    expect(result.error).toBeUndefined();

    const data = JSON.parse(result.value || "[]");
    expect(Array.isArray(data)).toBe(true);
    expect(data).toHaveLength(1);
    expect(data[0].count).toBe(2);
  });

  it("doesn't split semicolons inside strings or comments", async () => {
    await db.query(
      `CREATE TABLE semi_ui (id INTEGER PRIMARY KEY, name TEXT, note TEXT)`,
    );

    const multi = `
      INSERT INTO semi_ui (name, note) VALUES ('A', 'hello; world');
      /* block; comment; with; semicolons */
      INSERT INTO semi_ui (name, note) VALUES ('B', '-- not a comment ; inside string');
      `;

    const res = await db.query(multi);
    expect(res.error).toBeUndefined();

    const rows = await db.query(`SELECT name, note FROM semi_ui ORDER BY id`);
    const data = JSON.parse(rows.value || "[]");
    expect(data).toHaveLength(2);
    expect(data[0].name).toBe("A");
    expect(data[0].note).toBe("hello; world");
    expect(data[1].name).toBe("B");
    expect(data[1].note).toBe("-- not a comment ; inside string");
  });

  it("gates multi-statement execution by trailing semicolon", async () => {
    await db.query(`CREATE TABLE gate_ui (id INTEGER)`);

    const gated = `INSERT INTO gate_ui (id) VALUES (1); DELETE FROM gate_ui WHERE id = 1`;
    const res = await db.query(gated);

    expect(res.error).toBeUndefined();
    expect(res.value || "").toContain("Rows affected: 1");

    await assertions.assertRowCount(db, "SELECT * FROM gate_ui", 1);
  });

  it("supports triggers with semicolons in the body (BEGIN ... END)", async () => {
    await db.query(`CREATE TABLE trg_src_ui (id INTEGER)`);
    await db.query(`CREATE TABLE trg_log_ui (msg TEXT)`);

    const triggerSql = `
      CREATE TRIGGER trg_ui AFTER INSERT ON trg_src_ui
      BEGIN
        INSERT INTO trg_log_ui (msg) VALUES ('first; message');
        INSERT INTO trg_log_ui (msg) VALUES ('second; message');
      END;
    `;

    const createRes = await db.query(triggerSql);
    expect(createRes.error).toBeUndefined();

    await db.query(`INSERT INTO trg_src_ui (id) VALUES (1)`);

    const logRows = await db.query(`SELECT msg FROM trg_log_ui ORDER BY rowid`);
    const logData = JSON.parse(logRows.value || "[]");
    expect(logData).toHaveLength(2);
    expect(logData[0].msg).toBe("first; message");
    expect(logData[1].msg).toBe("second; message");
  });
});

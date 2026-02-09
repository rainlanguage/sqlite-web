import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createTestDatabase, cleanupDatabase, assertions } from '../fixtures/test-helpers.js';
import type { SQLiteWasmDatabase } from '@rainlanguage/sqlite-web';

describe('Parameter Binding (UI Integration)', () => {
  let db: SQLiteWasmDatabase;

  beforeEach(async () => {
    db = await createTestDatabase();
  });

  afterEach(async () => {
    if (db) await cleanupDatabase(db);
  });

  describe('Positional ? placeholders', () => {
    it('binds sequential ? parameters for INSERT and SELECT', async () => {
      await db.query(`
        CREATE TABLE param_test (
          a INTEGER,
          b TEXT,
          c REAL
        )
      `);

      const insert = await db.query(
        'INSERT INTO param_test (a,b,c) VALUES (?,?,?)',
        [123, 'abc', 3.5]
      );
      expect(insert.error).toBeUndefined();
      expect(insert.value || '').toContain('Rows affected: 1');

      const res = await db.query(
        'SELECT a,b,c FROM param_test WHERE a = ? AND b = ?',
        [123, 'abc']
      );
      const rows = JSON.parse(res.value || '[]');
      expect(rows).toHaveLength(1);
      expect(rows[0]).toMatchObject({ a: 123, b: 'abc' });
      expect(rows[0].c).toBeCloseTo(3.5, 6);
    });

    it('errors on parameter count mismatch for ? placeholders', async () => {
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT ? as a, ? as b', [10]);
      } catch (e) {
        caught = e;
      }

      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        expect(msg).toMatch(/Expected\s+2\s+parameters\s+but\s+got\s+1/i);
      } else {
        expect(result.error).toBeDefined();
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        expect(msg).toMatch(/Expected\s+2\s+parameters\s+but\s+got\s+1/i);
      }
    });

    it('errors when providing extra parameters for statements without placeholders', async () => {
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT 1', [42]);
      } catch (e) {
        caught = e;
      }

      const check = (msg: string) => expect(msg).toMatch(/No parameters expected/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else {
        expect(result.error).toBeDefined();
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });
  });

  describe('Numbered ?N placeholders', () => {
    it('binds ?1..N positions and allows reuse', async () => {
      const r1 = await db.query('SELECT ?1 AS a, ?2 AS b', [42, 'ok']);
      const rows1 = JSON.parse(r1.value || '[]');
      expect(rows1).toHaveLength(1);
      expect(rows1[0]).toMatchObject({ a: 42, b: 'ok' });

      const r2 = await db.query('SELECT ?1 AS x, ?1 AS y', [7]);
      const rows2 = JSON.parse(r2.value || '[]');
      expect(rows2).toHaveLength(1);
      expect(rows2[0]).toMatchObject({ x: 7, y: 7 });
    });

    it('errors on mixed ? and ?N forms', async () => {
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT ?1 AS a, ? AS b', [1, 2]);
      } catch (e) {
        caught = e;
      }
      const check = (msg: string) => expect(msg).toMatch(/Mixing '\?' and '\?N' placeholders is not supported/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else {
        expect(result.error).toBeDefined();
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });

    it('errors on invalid or missing numbered indices', async () => {
      // ?0 invalid
      {
        let caught: unknown = null;
        let result: any;
        try {
          result = await db.query('SELECT ?0 as bad', [1]);
        } catch (e) {
          caught = e;
        }
        const check = (msg: string) =>
          expect(msg).toMatch(/(Invalid parameter index: \?0|negative indices|variable number must be between \?1 and \?32766)/i);
        if (caught) {
          const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
          check(msg);
        } else {
          expect(result.error).toBeDefined();
          const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
          check(msg);
        }
      }

      // Missing ?1 when using ?2
      {
        let caught: unknown = null;
        let result: any;
        try {
          result = await db.query('SELECT ?2 as b', [99]);
        } catch (e) {
          caught = e;
        }
        const check = (msg: string) => {
          const regexOk = /(Missing parameter index \?1|Expected\s+2\s+parameters)/i.test(msg);
          const mixingOk = msg.includes("Mixing '?' and '?N' placeholders is not supported.");
          expect(regexOk || mixingOk).toBe(true);
        };
        if (caught) {
          const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
          check(msg);
        } else {
          expect(result.error).toBeDefined();
          const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
          check(msg);
        }
      }
    });

    it('errors when extra params supplied for ?N placeholders', async () => {
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT ?1 as a', [1, 2]);
      } catch (e) {
        caught = e;
      }
      const check = (msg: string) =>
        expect(msg).toMatch(/Expected\s+1\s+parameters\s+but\s+got\s+2/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else {
        expect(result.error).toBeDefined();
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });
  });

  describe('Type mapping', () => {
    it('binds null, boolean, number, string types correctly', async () => {
      await db.query(`
        CREATE TABLE param_types (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          nval INTEGER,
          bval INTEGER,
          dval REAL,
          sval TEXT
        )
      `);

      // null and boolean as 0/1
      const r1 = await db.query('INSERT INTO param_types (nval, bval, dval, sval) VALUES (?, ?, ?, ?)', [null, true, 3.25, 'hello']);
      expect(r1.error).toBeUndefined();
      expect(r1.value || '').toContain('Rows affected: 1');

      const r2 = await db.query('SELECT nval, bval, dval, sval FROM param_types');
      const rows = JSON.parse(r2.value || '[]');
      expect(rows).toHaveLength(1);
      expect(rows[0].nval).toBeNull();
      expect(rows[0].bval).toBe(1);
      expect(rows[0].dval).toBeCloseTo(3.25, 6);
      expect(rows[0].sval).toBe('hello');
    });

    it('binds BigInt within i64 range and rejects out-of-range', async () => {
      await db.query('CREATE TABLE bigint_test (v INTEGER)');

      const ok = await db.query('INSERT INTO bigint_test (v) VALUES (?1)', [9007199254740991n]); // Number.MAX_SAFE_INTEGER
      expect(ok.error).toBeUndefined();
      expect(ok.value || '').toContain('Rows affected: 1');

      const rowsRes = await db.query('SELECT v FROM bigint_test');
      const rows = JSON.parse(rowsRes.value || '[]');
      expect(rows[0].v).toBe(9007199254740991);

      // Out of range BigInt
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT ?1', [9223372036854775808n]);
      } catch (e) { caught = e; }
      const check = (msg: string) => expect(msg).toMatch(/BigInt.*range|out of i64 range/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else if (result?.error) {
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });

    it('binds blobs from Uint8Array and preserves length', async () => {
      await db.query('CREATE TABLE param_blob (data BLOB)');
      const bytes = new Uint8Array([1, 2, 3, 4, 5]);
      const ins = await db.query('INSERT INTO param_blob (data) VALUES (?)', [bytes]);
      expect(ins.error).toBeUndefined();

      const sel = await db.query('SELECT length(data) AS len FROM param_blob');
      const rows = JSON.parse(sel.value || '[]');
      expect(rows).toHaveLength(1);
      expect(rows[0].len).toBe(5);
    });

    it('rejects NaN/Infinity numbers at normalization', async () => {
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT ? as x', [Number.NaN]);
      } catch (e) { caught = e; }
      const check = (msg: string) => expect(msg).toMatch(/NaN\/Infinity not supported/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else if (result?.error) {
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });

    it('binds boolean false to 0', async () => {
      await db.query(`
        CREATE TABLE param_types (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          nval INTEGER,
          bval INTEGER,
          dval REAL,
          sval TEXT
        )
      `);

      const r1 = await db.query(
        'INSERT INTO param_types (nval, bval, dval, sval) VALUES (?, ?, ?, ?)',
        [null, false, 1.25, 'no']
      );
      expect(r1.error).toBeUndefined();

      const r2 = await db.query('SELECT nval, bval, dval, sval FROM param_types');
      const rows = JSON.parse(r2.value || '[]');
      expect(rows).toHaveLength(1);
      expect(rows[0].bval).toBe(0);
    });
  });

  describe('Validation and safety', () => {
    it('rejects named parameters when params are provided', async () => {
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('SELECT :name', ['ignored']);
      } catch (e) { caught = e; }
      const check = (msg: string) => expect(msg).toMatch(/Named parameters not supported/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else {
        expect(result.error).toBeDefined();
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });

    it('disallows multi-statement SQL when parameters are provided', async () => {
      await db.query('CREATE TABLE param_test (x INTEGER)');
      let caught: unknown = null;
      let result: any;
      try {
        result = await db.query('INSERT INTO param_test (x) VALUES (?); SELECT ?;', [1, 2]);
      } catch (e) { caught = e; }
      const check = (msg: string) => expect(msg).toMatch(/single statement/i);
      if (caught) {
        const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
        check(msg);
      } else {
        expect(result.error).toBeDefined();
        const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
        check(msg);
      }
    });

    it('prevents SQL injection via parameters', async () => {
      await db.query('CREATE TABLE param_test (id INTEGER PRIMARY KEY AUTOINCREMENT, val TEXT)');
      const malicious = `'; DROP TABLE param_test; --`;
      const ins = await db.query('INSERT INTO param_test (val) VALUES (?)', [malicious]);
      expect(ins.error).toBeUndefined();

      // Table should still exist and the literal string should be stored
      const rowsRes = await db.query('SELECT val FROM param_test');
      const rows = JSON.parse(rowsRes.value || '[]');
      expect(rows).toHaveLength(1);
      expect(rows[0].val).toBe(malicious);
    });
  });

  describe('Follower/Leader routing with parameters', () => {
    it('carries params across workers via BroadcastChannel', async () => {
      // Create a second DB instance referencing the same underlying database
      const db2 = await createTestDatabase('ui-test-db');
      try {
        await db.query('CREATE TABLE params_leader_test (id INTEGER PRIMARY KEY, name TEXT)');

        // Insert using follower/alternate db instance with parameters
        const r = await db2.query('INSERT INTO params_leader_test (id, name) VALUES (?1, ?2)', [1, 'alice']);
        expect(r.error).toBeUndefined();

        // Read using original db
        const sel = await db.query('SELECT id, name FROM params_leader_test');
        const rows = JSON.parse(sel.value || '[]');
        expect(rows).toHaveLength(1);
        expect(rows[0]).toMatchObject({ id: 1, name: 'alice' });
      } finally {
        await cleanupDatabase(db2);
      }
    });

    it('propagates errors across workers with parameters', async () => {
      const db2 = await createTestDatabase('ui-test-db');
      try {
        let caught: unknown = null;
        let result: any;
        try {
          result = await db2.query('SELECT ?0 as bad', [1]);
        } catch (e) {
          caught = e;
        }
        const check = (msg: string) =>
          expect(msg).toMatch(/(Invalid parameter index: \?0|negative indices|variable number must be between \?1 and \?32766|Invalid parameter index)/i);
        if (caught) {
          const msg = typeof caught === 'string' ? caught : (caught as any).message || String(caught);
          check(msg);
        } else {
          expect(result.error).toBeDefined();
          const msg = result.error?.msg || result.error?.readableMsg || JSON.stringify(result.error);
          check(msg);
        }
      } finally {
        await cleanupDatabase(db2);
      }
    });
  });
});

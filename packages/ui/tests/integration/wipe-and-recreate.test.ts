import { describe, it, expect, beforeAll } from 'vitest';
import { createTestDatabase } from '../fixtures/test-helpers.js';
import type { SQLiteWasmDatabase } from '@rainlanguage/sqlite-web';

describe.sequential('wipeAndRecreate', () => {
	let db: SQLiteWasmDatabase;

	beforeAll(async () => {
		db = await createTestDatabase(`test-wipe-${Date.now()}`);
	});

	it('should clear all tables after wipeAndRecreate', async () => {
		await db.query('CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)');
		await db.query("INSERT INTO users (name) VALUES ('Alice'), ('Bob')");

		let result = await db.query('SELECT COUNT(*) as count FROM users');
		const data = JSON.parse(result.value!);
		expect(data[0].count).toBe(2);

		const wipeResult = await db.wipeAndRecreate();
		expect(wipeResult.error).toBeUndefined();

		result = await db.query('SELECT * FROM users');
		expect(result.error).toBeDefined();
		expect(result.error!.msg).toContain('no such table');
	});

	it('should clear multiple tables', async () => {
		await db.query('CREATE TABLE t1 (id INTEGER)');
		await db.query('CREATE TABLE t2 (id INTEGER)');
		await db.query('CREATE TABLE t3 (id INTEGER)');
		await db.query('INSERT INTO t1 VALUES (1)');
		await db.query('INSERT INTO t2 VALUES (2)');
		await db.query('INSERT INTO t3 VALUES (3)');

		await db.wipeAndRecreate();

		for (const table of ['t1', 't2', 't3']) {
			const result = await db.query(`SELECT * FROM ${table}`);
			expect(result.error).toBeDefined();
		}
	});

	it('should allow creating new tables after wipe', async () => {
		await db.query('CREATE TABLE old (id INTEGER)');
		await db.wipeAndRecreate();

		const result = await db.query(
			'CREATE TABLE new_table (id INTEGER PRIMARY KEY, value TEXT)'
		);
		expect(result.error).toBeUndefined();
		expect(result.value).toContain('successfully');
	});

	it('should allow full CRUD operations after wipe', async () => {
		await db.wipeAndRecreate();

		await db.query('CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)');

		await db.query("INSERT INTO items (name) VALUES ('item1')");

		let result = await db.query('SELECT * FROM items');
		expect(result.value).toContain('item1');

		await db.query("UPDATE items SET name = 'updated' WHERE id = 1");
		result = await db.query('SELECT * FROM items');
		expect(result.value).toContain('updated');

		await db.query('DELETE FROM items WHERE id = 1');
		result = await db.query('SELECT COUNT(*) as count FROM items');
		const countData = JSON.parse(result.value!);
		expect(countData[0].count).toBe(0);
	});

	it('should support parameterized queries after wipe', async () => {
		await db.wipeAndRecreate();

		await db.query('CREATE TABLE params_test (id INTEGER, value TEXT)');

		const result = await db.query(
			'INSERT INTO params_test (id, value) VALUES (?, ?)',
			[42, 'parameterized']
		);
		expect(result.error).toBeUndefined();

		const selectResult = await db.query(
			'SELECT * FROM params_test WHERE id = ?',
			[42]
		);
		expect(selectResult.value).toContain('parameterized');
	});

	it('should keep same instance valid after wipe', async () => {
		const originalDb = db;

		await db.wipeAndRecreate();

		const result = await originalDb.query('SELECT 1 + 1 as sum');
		expect(result.error).toBeUndefined();
		const sumData = JSON.parse(result.value!);
		expect(sumData[0].sum).toBe(2);
	});

	it('should allow multiple consecutive wipes', async () => {
		for (let i = 0; i < 5; i++) {
			await db.query(`CREATE TABLE iter_${i} (id INTEGER)`);
			const wipeResult = await db.wipeAndRecreate();
			expect(wipeResult.error).toBeUndefined();
		}

		const result = await db.query('SELECT sqlite_version()');
		expect(result.error).toBeUndefined();
	});

	it('should recover from corrupted state simulation', async () => {
		await db.query('CREATE TABLE important_data (id INTEGER, value TEXT)');
		await db.query("INSERT INTO important_data VALUES (1, 'critical')");

		const result = await db.wipeAndRecreate();
		expect(result.error).toBeUndefined();

		await db.query('CREATE TABLE important_data (id INTEGER, value TEXT)');
		await db.query("INSERT INTO important_data VALUES (1, 'recovered')");

		const selectResult = await db.query('SELECT * FROM important_data');
		expect(selectResult.value).toContain('recovered');
	});
});

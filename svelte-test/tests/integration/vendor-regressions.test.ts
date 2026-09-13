import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { SQLiteWasmDatabase } from '@rainlanguage/sqlite-web';
import { cleanupDatabase, createTestDatabase } from '../fixtures/test-helpers.js';

describe('vendored sqlite runtime regressions', () => {
	let db: SQLiteWasmDatabase;

	beforeEach(async () => {
		db = await createTestDatabase();
	});

	afterEach(async () => {
		await cleanupDatabase(db);
	});

	it('converts an ordinary Unix timestamp to local time', async () => {
		const result = await db.query(
			"SELECT datetime(0, 'unixepoch', 'localtime') AS local_time"
		);
		const rows = JSON.parse(result.value || '[]') as Array<{ local_time: string }>;

		expect(rows).toHaveLength(1);
		expect(rows[0].local_time).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
	});
});

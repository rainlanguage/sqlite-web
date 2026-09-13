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

	it('uses the configured local offset and daylight-saving rules', async () => {
		const result = await db.query(
			`SELECT
				datetime(0, 'unixepoch', 'localtime') AS winter_time,
				datetime(1593561600, 'unixepoch', 'localtime') AS summer_time`
		);
		const rows = JSON.parse(result.value || '[]') as Array<{
			winter_time: string;
			summer_time: string;
		}>;

		expect(rows).toHaveLength(1);
		expect(rows[0]).toEqual({
			winter_time: '1969-12-31 19:00:00',
			summer_time: '2020-06-30 20:00:00'
		});
	});
});

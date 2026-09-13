import { describe, expect, it } from 'vitest';
import { createTestDatabase } from '../fixtures/test-helpers.js';

const SNAPSHOT_SHA256 = '5d5408c09e537d8af271fff0eab22ab9ef624259476010ada25de6a3732dec08';
const SNAPSHOT_SIZE = 1024;

describe('streaming SQLite snapshot installation', () => {
	it('installs uncompressed and gzip-compressed SQLite snapshots', async () => {
		const db = await createTestDatabase(`snapshot-raw-poc-${Date.now()}`);
		const rawInstall = await db.installSnapshot(
			new URL('/snapshot.raw.db', window.location.href).href,
			'none',
			SNAPSHOT_SHA256,
			SNAPSHOT_SIZE
		);

		expect(rawInstall.error).toBeUndefined();
		const rawStats = JSON.parse(rawInstall.value || '{}');
		expect(rawStats).toMatchObject({
			bytesWritten: SNAPSHOT_SIZE,
			compression: 'none',
			sha256: SNAPSHOT_SHA256
		});

		const rawResult = await db.query(
			"SELECT COUNT(*) AS table_count FROM sqlite_schema WHERE type = 'table'"
		);
		expect(rawResult.error).toBeUndefined();
		const [rawRow] = JSON.parse(rawResult.value || '[]');
		expect(rawRow.table_count).toBe(1);
		const rawItems = await db.query('SELECT id, label FROM snapshot_items ORDER BY id');
		expect(rawItems.error).toBeUndefined();
		expect(JSON.parse(rawItems.value || '[]')).toEqual([
			{ id: 1, label: 'alpha' },
			{ id: 2, label: 'beta' },
			{ id: 3, label: 'gamma' }
		]);

		const sentinelResult = await db.query(`
			CREATE TABLE snapshot_install_sentinel (value TEXT NOT NULL);
			INSERT INTO snapshot_install_sentinel VALUES ('original database');
		`);
		expect(sentinelResult.error).toBeUndefined();

		const rejectedInstall = await db.installSnapshot(
			new URL('/snapshot.raw.db', window.location.href).href,
			'none',
			'0'.repeat(64),
			SNAPSHOT_SIZE
		);
		expect(rejectedInstall.error?.msg).toContain('SHA-256 mismatch');

		const preservedResult = await db.query(
			'SELECT value FROM snapshot_install_sentinel'
		);
		expect(preservedResult.error).toBeUndefined();
		expect(JSON.parse(preservedResult.value || '[]')).toEqual([
			{ value: 'original database' }
		]);

		const install = await db.installSnapshot(
			new URL('/snapshot.db.gz', window.location.href).href,
			'gzip',
			SNAPSHOT_SHA256,
			SNAPSHOT_SIZE
		);
		expect(install.error).toBeUndefined();
		const stats = JSON.parse(install.value || '{}');
		expect(stats.bytesWritten).toBe(SNAPSHOT_SIZE);
		expect(stats.compression).toBe('gzip');
		expect(stats.sha256).toBe(SNAPSHOT_SHA256);

		const result = await db.query(`
			SELECT
				(SELECT quick_check FROM pragma_quick_check LIMIT 1) AS quick_check,
				(SELECT page_count FROM pragma_page_count) AS page_count,
				(SELECT page_size FROM pragma_page_size) AS page_size,
				(SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table') AS table_count,
				(SELECT COUNT(*) FROM sqlite_schema WHERE name = 'snapshot_install_sentinel') AS sentinel_count
		`);
		expect(result.error).toBeUndefined();
		const [row] = JSON.parse(result.value || '[]');
		expect(row.quick_check).toBe('ok');
		expect(row.page_count * row.page_size).toBe(SNAPSHOT_SIZE);
		expect(row.table_count).toBe(1);
		expect(row.sentinel_count).toBe(0);
	});
});

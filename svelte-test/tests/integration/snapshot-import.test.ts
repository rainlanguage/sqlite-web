import { afterEach, describe, expect, it } from 'vitest';
import { createTestDatabase } from '../fixtures/test-helpers.js';

const SNAPSHOT_SHA256 = '5d5408c09e537d8af271fff0eab22ab9ef624259476010ada25de6a3732dec08';
const SNAPSHOT_SIZE = 1024;
const CORRUPT_SNAPSHOT_SHA256 = 'ba23dd3c57c8f955a8abe95509623b444b68b2018c36730153e50a28e352ebd2';
type TestDatabase = Awaited<ReturnType<typeof createTestDatabase>>;
const openDatabases: TestDatabase[] = [];

async function createSnapshotDatabase(name: string): Promise<TestDatabase> {
	const database = await createTestDatabase(name);
	openDatabases.push(database);
	return database;
}

afterEach(async () => {
	for (const database of openDatabases.splice(0).reverse()) {
		await database.close();
		database.free();
	}
});

describe('streaming SQLite snapshot installation', () => {
	it('opens a long VFS-valid name while rejecting snapshot staging paths', async () => {
		const db = await createSnapshotDatabase('l'.repeat(480));
		const write = await db.query(`
			CREATE TABLE long_name_sentinel (value TEXT NOT NULL);
			INSERT INTO long_name_sentinel VALUES ('ordinary open works');
		`);
		expect(write.error).toBeUndefined();

		const install = await db.installSnapshot(
			new URL('/snapshot.raw.db', window.location.href).href,
			'none',
			SNAPSHOT_SHA256,
			SNAPSHOT_SIZE
		);
		expect(install.error?.msg).toContain('too long for atomic snapshot installation');

		const read = await db.query('SELECT value FROM long_name_sentinel');
		expect(JSON.parse(read.value || '[]')).toEqual([{ value: 'ordinary open works' }]);
	});

	it('installs uncompressed and gzip-compressed SQLite snapshots', async () => {
		const db = await createSnapshotDatabase(`snapshot-raw-poc-${Date.now()}`);
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

		const corruptInstall = await db.installSnapshot(
			new URL('/snapshot.corrupt.db', window.location.href).href,
			'none',
			CORRUPT_SNAPSHOT_SHA256,
			SNAPSHOT_SIZE
		);
		expect(corruptInstall.error?.msg).toContain('integrity validation');
		const preservedAfterCorruption = await db.query(
			'SELECT value FROM snapshot_install_sentinel'
		);
		expect(JSON.parse(preservedAfterCorruption.value || '[]')).toEqual([
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

	it('cancels an oversized response and remains usable', async () => {
		await fetch('/snapshot-stats?reset=1');
		const db = await createSnapshotDatabase(`snapshot-cancel-${Date.now()}`);
		const rejected = await db.installSnapshot(
			new URL('/snapshot.oversize.db', window.location.href).href,
			'none',
			'0'.repeat(64),
			SNAPSHOT_SIZE
		);
		expect(rejected.error?.msg).toContain('exceeded expected size');

		for (let i = 0; i < 20; i += 1) {
			const stats = await fetch('/snapshot-stats').then((response) => response.json());
			if (stats.cancelledSnapshotRequestCount > 0) break;
			await new Promise((resolve) => setTimeout(resolve, 25));
		}
		const stats = await fetch('/snapshot-stats').then((response) => response.json());
		expect(stats.cancelledSnapshotRequestCount).toBeGreaterThan(0);

		const successful = await db.installSnapshot(
			new URL('/snapshot.raw.db', window.location.href).href,
			'none',
			SNAPSHOT_SHA256,
			SNAPSHOT_SIZE
		);
		expect(successful.error).toBeUndefined();
	});

	it('coalesces matching installs from two clients into one download', async () => {
		await fetch('/snapshot-stats?reset=1');
		const name = `snapshot-coalesce-${Date.now()}`;
		const [leader, follower] = await Promise.all([
			createSnapshotDatabase(name),
			createSnapshotDatabase(name)
		]);
		const url = new URL('/snapshot.delayed.db', window.location.href).href;
		const [first, second] = await Promise.all([
			leader.installSnapshot(url, 'none', SNAPSHOT_SHA256, SNAPSHOT_SIZE),
			follower.installSnapshot(url, 'none', SNAPSHOT_SHA256, SNAPSHOT_SIZE)
		]);
		expect(first.error).toBeUndefined();
		expect(second.error).toBeUndefined();
		const stats = await fetch('/snapshot-stats').then((response) => response.json());
		expect(stats.snapshotRequestCount).toBe(1);
	});
});

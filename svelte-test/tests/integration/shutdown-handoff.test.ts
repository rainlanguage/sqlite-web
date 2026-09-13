import { describe, expect, it } from 'vitest';
import type { SQLiteWasmDatabase } from '@rainlanguage/sqlite-web';
import { createTestDatabase } from '../fixtures/test-helpers.js';

const SNAPSHOT_SHA256 = '5d5408c09e537d8af271fff0eab22ab9ef624259476010ada25de6a3732dec08';
const DRAIN_REGRESSION_DATABASE = 'shutdown-drain-regressions';

async function closeAfterLeadershipHandoff(db: SQLiteWasmDatabase): Promise<void> {
	for (let attempt = 0; attempt < 100; attempt += 1) {
		const ready = await db.query('SELECT 1');
		if (!ready.error) {
			const closed = await db.close();
			expect(closed.error).toBeUndefined();
			return;
		}
		await new Promise((resolve) => setTimeout(resolve, 25));
	}
	throw new Error('surviving client did not complete leadership handoff');
}

describe('database worker shutdown handoff', () => {
	it('drains follower-only work before an otherwise-idle leader closes', async () => {
		const databaseName = DRAIN_REGRESSION_DATABASE;
		const leader = await createTestDatabase(databaseName);
		const follower = await createTestDatabase(databaseName);
		try {
			await fetch('/snapshot-stats?reset=1');
			const followerInstall = follower.installSnapshot(
				new URL('/snapshot.slow.db', window.location.href).href,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			for (let attempt = 0; attempt < 40; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.snapshotRequestCount === 1) break;
				await new Promise((resolve) => setTimeout(resolve, 25));
			}

			const close = leader.close();
			const followerResult = await Promise.race([
				followerInstall,
				new Promise<never>((_, reject) =>
					setTimeout(() => reject(new Error('follower-only install did not settle')), 3000)
				)
			]);
			const closeResult = await Promise.race([
				close,
				new Promise<never>((_, reject) =>
					setTimeout(() => reject(new Error('follower-only leader close did not settle')), 3000)
				)
			]);
			expect(followerResult.error).toBeUndefined();
			expect(closeResult.error).toBeUndefined();
			const stats = await fetch('/snapshot-stats').then((response) => response.json());
			expect(stats.snapshotRequestCount).toBe(1);
		} finally {
			leader.free();
			await closeAfterLeadershipHandoff(follower);
			follower.free();
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	});

	it('drains unrelated follower work during an immediate local install and close', async () => {
		const databaseName = DRAIN_REGRESSION_DATABASE;
		const leader = await createTestDatabase(databaseName);
		const follower = await createTestDatabase(databaseName);
		try {
			await fetch('/snapshot-stats?reset=1');
			const followerInstall = follower.installSnapshot(
				new URL('/snapshot.slow.db', window.location.href).href,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			for (let attempt = 0; attempt < 40; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.snapshotRequestCount === 1) break;
				await new Promise((resolve) => setTimeout(resolve, 25));
			}

			const localInstall = leader.installSnapshot(
				new URL('/snapshot.delayed.db?local=1', window.location.href).href,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			const [localResult, followerResult, closeResult] = await Promise.race([
				Promise.all([localInstall, followerInstall, leader.close()]),
				new Promise<never>((_, reject) =>
					setTimeout(() => reject(new Error('cancel-before-request drain did not settle')), 3000)
				)
			]);
			expect(localResult.error?.msg).toContain('Snapshot installation cancelled');
			expect(followerResult.error).toBeUndefined();
			expect(closeResult.error).toBeUndefined();
		} finally {
			leader.free();
			await closeAfterLeadershipHandoff(follower);
			follower.free();
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	});

	it('settles a follower arriving while the sole leader install is cancelling', async () => {
		const databaseName = DRAIN_REGRESSION_DATABASE;
		const leader = await createTestDatabase(databaseName);
		const follower = await createTestDatabase(databaseName);
		try {
			await fetch('/snapshot-stats?reset=1');
			const url = new URL('/snapshot.slow.db', window.location.href).href;
			const leaderInstall = leader.installSnapshot(
				url,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			for (let attempt = 0; attempt < 40; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.snapshotRequestCount === 1) break;
				await new Promise((resolve) => setTimeout(resolve, 25));
			}

			const close = leader.close();
			await Promise.resolve();
			const followerInstall = follower.installSnapshot(
				url,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			const [leaderResult, followerResult, closeResult] = await Promise.race([
				Promise.all([leaderInstall, followerInstall, close]),
				new Promise<never>((_, reject) =>
					setTimeout(() => reject(new Error('successor ordering did not settle')), 3000)
				)
			]);
			expect(leaderResult.error?.msg).toContain('Snapshot installation cancelled');
			expect(closeResult.error).toBeUndefined();
			expect(followerResult.error?.msg).toContain(
				'Leader is shutting down; retry on the next leader'
			);
			const stats = await fetch('/snapshot-stats').then((response) => response.json());
			expect(stats.snapshotRequestCount).toBe(1);
		} finally {
			leader.free();
			await closeAfterLeadershipHandoff(follower);
			follower.free();
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	});

	it('settles install and close when cancellation arrives before forwarding', async () => {
		const databaseName = `shutdown-immediate-close-${Date.now()}`;
		const leader = await createTestDatabase(databaseName);
		const follower = await createTestDatabase(databaseName);
		try {
			const write = await leader.query(`
				CREATE TABLE immediate_close_sentinel (value TEXT NOT NULL);
				INSERT INTO immediate_close_sentinel VALUES ('preserved');
			`);
			expect(write.error).toBeUndefined();
			const install = follower.installSnapshot(
				new URL('/snapshot.slow.db', window.location.href).href,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			const close = follower.close();
			const [installResult, closeResult] = await Promise.race([
				Promise.all([install, close]),
				new Promise<never>((_, reject) =>
					setTimeout(() => reject(new Error('immediate close did not settle')), 2000)
				)
			]);
			expect(installResult.error?.msg).toContain('Snapshot installation cancelled');
			expect(closeResult.error).toBeUndefined();
			const read = await leader.query('SELECT value FROM immediate_close_sentinel');
			expect(JSON.parse(read.value || '[]')).toEqual([{ value: 'preserved' }]);
		} finally {
			follower.free();
			await leader.close();
			leader.free();
		}
	});

	it('keeps a closing leader alive for a coalesced follower install', async () => {
		const databaseName = `shutdown-leader-coalesced-${Date.now()}`;
		const leader = await createTestDatabase(databaseName);
		const follower = await createTestDatabase(databaseName);
		try {
			await fetch('/snapshot-stats?reset=1');
			const url = new URL('/snapshot.slow.db', window.location.href).href;
			const leaderInstall = leader.installSnapshot(
				url,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			const followerInstall = follower.installSnapshot(
				url,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			for (let attempt = 0; attempt < 40; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.snapshotRequestCount === 1) break;
				await new Promise((resolve) => setTimeout(resolve, 25));
			}
			const close = leader.close();
			const [leaderResult, followerResult, closeResult] = await Promise.all([
				leaderInstall,
				followerInstall,
				close
			]);
			expect(leaderResult.error?.msg).toContain('Snapshot installation cancelled');
			expect(followerResult.error).toBeUndefined();
			expect(closeResult.error).toBeUndefined();
			const stats = await fetch('/snapshot-stats').then((response) => response.json());
			expect(stats.snapshotRequestCount).toBe(1);
			let read = await follower.query('SELECT label FROM snapshot_items ORDER BY id');
			for (let attempt = 0; read.error && attempt < 80; attempt += 1) {
				await new Promise((resolve) => setTimeout(resolve, 25));
				read = await follower.query('SELECT label FROM snapshot_items ORDER BY id');
			}
			expect(read.error).toBeUndefined();
		} finally {
			leader.free();
			await closeAfterLeadershipHandoff(follower);
			follower.free();
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
	});

	it('fences follower cancellation during snapshot finalization', async () => {
		const databaseName = `shutdown-follower-finalize-${Date.now()}`;
		const leader = await createTestDatabase(databaseName);
		let follower: SQLiteWasmDatabase | undefined;
		try {
			const write = await leader.query(`
				CREATE TABLE finalization_sentinel (value TEXT NOT NULL);
				INSERT INTO finalization_sentinel VALUES ('must survive finalization');
			`);
			expect(write.error).toBeUndefined();
			follower = await createTestDatabase(databaseName);
			await fetch('/snapshot-stats?reset=1');
			const metadata = await fetch('/snapshot-finalization-meta').then((response) =>
				response.json()
			);

			const install = follower.installSnapshot(
				new URL('/snapshot.finalization.db', window.location.href).href,
				'none',
				metadata.sha256,
				metadata.size
			);
			for (let attempt = 0; attempt < 400; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.completedSnapshotRequestCount === 1) break;
				await new Promise((resolve) => setTimeout(resolve, 2));
			}
			const completed = await fetch('/snapshot-stats').then((response) => response.json());
			expect(completed.completedSnapshotRequestCount).toBe(1);

			const closed = await follower.close();
			expect(closed.error).toBeUndefined();
			const installResult = await install;
			expect(installResult.error?.msg).toContain('Snapshot installation cancelled');
			follower.free();
			follower = undefined;

			const read = await leader.query('SELECT value FROM finalization_sentinel');
			expect(read.error).toBeUndefined();
			expect(JSON.parse(read.value || '[]')).toEqual([
				{ value: 'must survive finalization' }
			]);
		} finally {
			if (follower) {
				await follower.close();
				follower.free();
			}
			await leader.close();
			leader.free();
		}
	}, 30000);

	it('detaches a closing follower and cancels an orphaned leader install', async () => {
		const databaseName = `shutdown-follower-snapshot-${Date.now()}`;
		const leader = await createTestDatabase(databaseName);
		let follower: SQLiteWasmDatabase | undefined;
		try {
			const write = await leader.query(`
				CREATE TABLE follower_close_sentinel (value TEXT NOT NULL);
				INSERT INTO follower_close_sentinel VALUES ('must survive');
			`);
			expect(write.error).toBeUndefined();
			follower = await createTestDatabase(databaseName);
			await fetch('/snapshot-stats?reset=1');

			const install = follower.installSnapshot(
				new URL('/snapshot.slow.db', window.location.href).href,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			for (let attempt = 0; attempt < 40; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.snapshotRequestCount === 1) break;
				await new Promise((resolve) => setTimeout(resolve, 25));
			}
			const started = await fetch('/snapshot-stats').then((response) => response.json());
			expect(started.snapshotRequestCount).toBe(1);

			const closed = await follower.close();
			expect(closed.error).toBeUndefined();
			const installResult = await install;
			expect(installResult.error?.msg).toContain('Snapshot installation cancelled');
			follower.free();
			follower = undefined;

			for (let attempt = 0; attempt < 40; attempt += 1) {
				const stats = await fetch('/snapshot-stats').then((response) => response.json());
				if (stats.cancelledSnapshotRequestCount > 0) break;
				await new Promise((resolve) => setTimeout(resolve, 25));
			}
			const cancelled = await fetch('/snapshot-stats').then((response) => response.json());
			expect(cancelled.cancelledSnapshotRequestCount).toBeGreaterThan(0);

			const read = await leader.query('SELECT value FROM follower_close_sentinel');
			expect(read.error).toBeUndefined();
			expect(JSON.parse(read.value || '[]')).toEqual([{ value: 'must survive' }]);
		} finally {
			if (follower) {
				await follower.close();
				follower.free();
			}
			await leader.close();
			leader.free();
		}
	});

	it('cannot resurrect a worker when close races wipeAndRecreate', async () => {
		const database = await createTestDatabase(`shutdown-wipe-race-${Date.now()}`);
		try {
			const wipe = database.wipeAndRecreate();
			// wipeAndRecreate has stopped the old worker and yielded while deleting
			// OPFS. Closing now permanently invalidates that wipe generation.
			await Promise.resolve();
			const closed = await database.close();
			expect(closed.error).toBeUndefined();

			const wipeResult = await wipe;
			expect(wipeResult.error?.msg).toContain('Database is closed');
			const query = await database.query('SELECT 1 AS value');
			expect(query.error?.msg).toContain('Database is closed');
		} finally {
			await database.close();
			database.free();
		}
	});

	it('closes a live worker before a second client reopens the same OPFS database', async () => {
		const databaseName = `shutdown-handoff-${Date.now()}`;
		let first: SQLiteWasmDatabase | undefined = await createTestDatabase(databaseName);
		let second: SQLiteWasmDatabase | undefined;

		try {
			const write = await first.query(`
				CREATE TABLE shutdown_handoff_sentinel (value TEXT NOT NULL);
				INSERT INTO shutdown_handoff_sentinel VALUES ('persisted before close');
			`);
			expect(write.error).toBeUndefined();

			// Leave a request outstanding to prove close settles public promises,
			// rather than merely making future calls fail.
			const pendingInstall = first.installSnapshot(
				new URL('/snapshot.delayed.db', window.location.href).href,
				'none',
				SNAPSHOT_SHA256,
				1024
			);
			await Promise.resolve();

			const closed = await first.close();
			expect(closed.error).toBeUndefined();

			const pendingResult = await pendingInstall;
			expect(pendingResult.error?.msg).toMatch(
				/Database closed|Snapshot installation cancelled/
			);
			first.free();
			first = undefined;

			// The document remains alive. Only the first SDK worker is gone.
			second = await createTestDatabase(databaseName);
			const read = await second.query('SELECT value FROM shutdown_handoff_sentinel');
			expect(read.error).toBeUndefined();
			expect(JSON.parse(read.value || '[]')).toEqual([
				{ value: 'persisted before close' }
			]);
		} finally {
			if (first) {
				await first.close();
				first.free();
			}
			if (second) {
				await second.close();
				second.free();
			}
		}
	});
});

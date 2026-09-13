import { defineConfig } from 'vitest/config';
import path from 'path';
import fs from 'fs';
import os from 'node:os';
import { gzipSync } from 'node:zlib';
import { createHash } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';

// A deterministic, two-page SQLite fixture with one `snapshot_items` table.
// Keeping it inline makes the browser integration test hermetic and avoids
// coupling the SDK suite to any consumer's production database dump.
const snapshotFixture = Buffer.from(
	[
		'U1FMaXRlIGZvcm1hdCAzAAIAAQEMQCAgAAAABAAAAAIAAAAAAAAAAAAAAAIAAAAEAAAAAAAAAAAAAAAB',
		'AAAABwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAC6N+A0AAAABAYEAAYEAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHEBBxcpKQGBHXRhYmxlc25hcHNob3RfaXRlbXNzbmFwc2hv',
		'dF9pdGVtcwJDUkVBVEUgVEFCTEUgc25hcHNob3RfaXRlbXMoaWQgSU5URUdFUiBQUklNQVJZIEtFWSwg',
		'bGFiZWwgVEVYVCBOT1QgTlVMTCkAAAAAAAAAAAAAAAANAAAAAwHXAAHqAeEB1wAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
		'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIAwMAF2dhbW1hBwIDABViZXRhCAEDABdhbHBoYQAAAAAAAAAA',
		'AAAAAA=='
	].join(''),
	'base64'
);
const compressedSnapshotFixture = gzipSync(snapshotFixture, { mtime: 0 });
const corruptSnapshotFixture = Buffer.from(snapshotFixture);
corruptSnapshotFixture[512] = 0;
const corruptSnapshotSha256 = createHash('sha256').update(corruptSnapshotFixture).digest('hex');
const finalizationFixtureDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'sqlite-web-finalize-'));
const finalizationFixturePath = path.join(finalizationFixtureDirectory, 'snapshot.db');
const finalizationDatabase = new DatabaseSync(finalizationFixturePath);
finalizationDatabase.exec(`
	PRAGMA journal_mode = OFF;
	CREATE TABLE snapshot_items(id INTEGER PRIMARY KEY, label TEXT NOT NULL);
	INSERT INTO snapshot_items VALUES (1, 'replacement snapshot');
	CREATE TABLE validation_padding(payload BLOB NOT NULL);
	INSERT INTO validation_padding VALUES (zeroblob(33554432));
`);
finalizationDatabase.close();
const finalizationSnapshotFixture = fs.readFileSync(finalizationFixturePath);
fs.rmSync(finalizationFixtureDirectory, { recursive: true });
const finalizationSnapshotSha256 = createHash('sha256')
	.update(finalizationSnapshotFixture)
	.digest('hex');
let snapshotRequestCount = 0;
let cancelledSnapshotRequestCount = 0;
let completedSnapshotRequestCount = 0;

export default defineConfig({
	plugins: [
		{
			name: 'rainlanguage-sqlite-web-serve',
			configureServer(server) {
				server.middlewares.use('/snapshot-stats', (req, res) => {
					if (req.url?.includes('reset')) {
						snapshotRequestCount = 0;
						cancelledSnapshotRequestCount = 0;
						completedSnapshotRequestCount = 0;
					}
					res.setHeader('Content-Type', 'application/json');
					res.end(
						JSON.stringify({
							snapshotRequestCount,
							cancelledSnapshotRequestCount,
							completedSnapshotRequestCount
						})
					);
				});
				server.middlewares.use('/snapshot-finalization-meta', (_req, res) => {
					res.setHeader('Content-Type', 'application/json');
					res.end(
						JSON.stringify({
							sha256: finalizationSnapshotSha256,
							size: finalizationSnapshotFixture.length
						})
					);
				});
				server.middlewares.use('/snapshot.finalization.db', (_req, res) => {
					snapshotRequestCount += 1;
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					res.setHeader('Content-Length', finalizationSnapshotFixture.length);
					res.end(finalizationSnapshotFixture, () => {
						completedSnapshotRequestCount += 1;
					});
				});
				server.middlewares.use('/snapshot.corrupt.db', (_req, res) => {
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					res.setHeader('X-Snapshot-Sha256', corruptSnapshotSha256);
					res.end(corruptSnapshotFixture);
				});
				server.middlewares.use('/snapshot.oversize.db', (req, res) => {
					let complete = false;
					const chunk = Buffer.alloc(64 * 1024, 1);
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					const interval = setInterval(() => res.write(chunk), 10);
					const finish = setTimeout(() => {
						complete = true;
						clearInterval(interval);
						res.end();
					}, 2000);
					req.on('close', () => {
						clearInterval(interval);
						clearTimeout(finish);
						if (!complete) cancelledSnapshotRequestCount += 1;
					});
				});
				server.middlewares.use('/snapshot.raw.db', (_req, res) => {
					snapshotRequestCount += 1;
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					res.setHeader('Content-Length', snapshotFixture.length);
					res.end(snapshotFixture);
				});
				server.middlewares.use('/snapshot.delayed.db', (_req, res) => {
					snapshotRequestCount += 1;
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					setTimeout(() => res.end(snapshotFixture), 150);
				});
				server.middlewares.use('/snapshot.slow.db', (req, res) => {
					snapshotRequestCount += 1;
					let complete = false;
					let offset = 0;
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					const interval = setInterval(() => {
						const next = Math.min(offset + 128, snapshotFixture.length);
						res.write(snapshotFixture.subarray(offset, next));
						offset = next;
						if (offset === snapshotFixture.length) {
							complete = true;
							clearInterval(interval);
							res.end();
						}
					}, 100);
					req.on('close', () => {
						clearInterval(interval);
						if (!complete) cancelledSnapshotRequestCount += 1;
					});
				});
				server.middlewares.use('/snapshot.db.gz', (_req, res) => {
					res.setHeader('Content-Type', 'application/gzip');
					res.setHeader('Content-Length', compressedSnapshotFixture.length);
					res.end(compressedSnapshotFixture);
				});
				server.middlewares.use('/pkg', (req, res, next) => {
					const filePath = req.url?.substring(1);
					const fullPath = path.join(process.cwd(), 'node_modules/@rainlanguage/sqlite-web', filePath || '');
					
					if (fs.existsSync(fullPath)) {
						if (fullPath.endsWith('.wasm')) {
							res.setHeader('Content-Type', 'application/wasm');
						} else if (fullPath.endsWith('.js')) {
							res.setHeader('Content-Type', 'application/javascript');
						}
						fs.createReadStream(fullPath).pipe(res);
					} else {
						next();
					}
				});
			}
		}
	],
	test: {
		// Browser mode configuration
		browser: {
			enabled: true,
			name: 'chromium',
			provider: 'playwright',
			// Enable necessary web APIs
			headless: true,
			// Allow access to OPFS, BroadcastChannel, etc.
			api: {
				port: 63315
			},
			// Add cross-origin isolation headers for SharedArrayBuffer and OPFS
			providerOptions: {
				context: {
					timezoneId: 'America/New_York'
				},
				launch: {
					args: [
						'--enable-features=SharedArrayBuffer',
						'--disable-web-security',
						'--allow-running-insecure-content',
						'--disable-features=VizDisplayCompositor'
					]
				}
			}
		},
		// Test configuration
		testTimeout: 30000, // 30 seconds for database operations
		hookTimeout: 10000, // 10 seconds for setup/teardown
		teardownTimeout: 10000,
		// Test file patterns  
		include: ['tests/**/*.test.{js,ts}'],
		exclude: [
			'**/node_modules/**',
			'**/dist/**',
			'**/.svelte-kit/**'
		],
		// Global test setup
		globalSetup: './tests/global-setup.js',
		// Environment setup for each test
		setupFiles: ['./tests/test-setup.js']
	},
	// Vite configuration for tests
	server: {
		fs: {
			allow: ['..']
		},
		headers: {
			'Cross-Origin-Embedder-Policy': 'require-corp',
			'Cross-Origin-Opener-Policy': 'same-origin'
		}
	},
	optimizeDeps: {
		exclude: ['@rainlanguage/sqlite-web']
	},
	assetsInclude: ['**/*.wasm'],
	// Resolve configuration
	resolve: {
		alias: {
			'$lib': path.resolve('./src/lib')
		}
	}
});

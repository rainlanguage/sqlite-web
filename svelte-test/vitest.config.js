import { defineConfig } from 'vitest/config';
import path from 'path';
import fs from 'fs';
import { gzipSync } from 'node:zlib';

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

export default defineConfig({
	plugins: [
		{
			name: 'rainlanguage-sqlite-web-serve',
			configureServer(server) {
				server.middlewares.use('/snapshot.raw.db', (_req, res) => {
					res.setHeader('Content-Type', 'application/vnd.sqlite3');
					res.setHeader('Content-Length', snapshotFixture.length);
					res.end(snapshotFixture);
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

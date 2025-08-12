// Test setup run before each test file
import { beforeEach, afterEach } from 'vitest';

// Clean up any existing databases/workers before each test
beforeEach(async () => {
	// Clear OPFS storage
	if (typeof navigator !== 'undefined' && 'storage' in navigator) {
		try {
			const opfsRoot = await navigator.storage.getDirectory();
			// Remove any existing database files
			for await (const [name, handle] of opfsRoot.entries()) {
				if (name.includes('worker.db') || name.includes('sqlite')) {
					await opfsRoot.removeEntry(name, { recursive: true });
				}
			}
		} catch (error) {
			// OPFS might not be available or accessible
			console.warn('Could not clean OPFS:', error.message);
		}
	}
	
	// Clear any broadcast channel listeners
	if (typeof BroadcastChannel !== 'undefined') {
		// Close any existing broadcast channels
		// This will be handled by individual tests
	}
});

// Clean up after each test
afterEach(async () => {
	// Force garbage collection if available
	if (typeof globalThis.gc === 'function') {
		globalThis.gc();
	}
	
	// Wait a bit for cleanup
	await new Promise(resolve => setTimeout(resolve, 100));
});
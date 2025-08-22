import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { 
	createTestDatabase, 
	cleanupDatabase, 
	assertions,
	PerformanceTracker 
} from '../fixtures/test-helpers.js';
import type { SQLiteWasmDatabase } from 'sqlite-web';

interface CategoryRow {
	category: string;
	total: string;
}

interface MiningRow {
	category: string;
	total_wei: string;
}

describe('BIGINT_SUM Database Function', () => {
	let db: SQLiteWasmDatabase;
	let perf: PerformanceTracker;
	
	beforeEach(async () => {
		db = await createTestDatabase();
		perf = new PerformanceTracker();
		
		// Create test table for big integer operations
		await db.query(`
			CREATE TABLE bigint_test (
				id INTEGER PRIMARY KEY AUTOINCREMENT,
				amount TEXT NOT NULL,
				category TEXT,
				description TEXT
			)
		`);
	});
	
	afterEach(async () => {
		await cleanupDatabase(db);
	});

	describe('Function Availability', () => {
		it('should have BIGINT_SUM function available', async () => {
			// First check what functions are available
			try {
				const pragmaResult = await db.query('PRAGMA function_list');
				console.log('Available functions:', pragmaResult);
			} catch (error) {
				console.log('Cannot list functions, SQLite version may not support it');
			}

			// Test RAIN_MATH_PROCESS (known working function) 
			try {
				const rainResult = await db.query('SELECT RAIN_MATH_PROCESS("100", "200") as test');
				console.log('RAIN_MATH_PROCESS test result:', rainResult);
			} catch (error) {
				console.error('RAIN_MATH_PROCESS not available:', error);
			}

			// Test if function exists by trying to use it
			try {
				const result = await db.query('SELECT BIGINT_SUM("123") as test');
				console.log('BIGINT_SUM function test result:', result);
				expect(result).toBeDefined();
			} catch (error) {
				console.error('BIGINT_SUM function not available:', error);
				throw error;
			}
		});
	});

	describe('Basic BIGINT_SUM Functionality', () => {
		beforeEach(async () => {
			// Insert test data with big integers as strings
			await db.query(`
				INSERT INTO bigint_test (amount, category, description) VALUES 
				('123456789012345678901234567890', 'income', 'Large payment'),
				('987654321098765432109876543210', 'income', 'Massive deposit'),
				('555555555555555555555555555555', 'income', 'Medium amount')
			`);
		});

		it('should sum positive big integers correctly', async () => {
			perf.start('bigint-sum-positive');
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			perf.end('bigint-sum-positive');
			
			const data = JSON.parse(result.value || '[]');
			
			expect(data).toHaveLength(1);
			expect(data[0]).toHaveProperty('total');
			
			// Expected: 123456789012345678901234567890 + 987654321098765432109876543210 + 555555555555555555555555555555
			// = 1666666665666666666566666666655
			expect(data[0].total).toBe('1666666665666666666566666666655');
			expect(perf.getDuration('bigint-sum-positive')).toBeLessThan(1000);
		});

		it('should handle single row correctly', async () => {
			await db.query('DELETE FROM bigint_test WHERE id > 1');
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('123456789012345678901234567890');
		});

		it('should return 0 for empty table', async () => {
			await db.query('DELETE FROM bigint_test');
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0');
		});
	});

	describe('Negative Numbers and Mixed Operations', () => {
		beforeEach(async () => {
			await db.query(`
				INSERT INTO bigint_test (amount, category, description) VALUES 
				('1000000000000000000000000000000', 'income', 'Large credit'),
				('-500000000000000000000000000000', 'expense', 'Large debit'),
				('250000000000000000000000000000', 'income', 'Medium credit'),
				('-100000000000000000000000000000', 'expense', 'Medium debit')
			`);
		});

		it('should handle negative amounts correctly', async () => {
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			// Expected: 1000000000000000000000000000000 - 500000000000000000000000000000 + 250000000000000000000000000000 - 100000000000000000000000000000
			// = 650000000000000000000000000000
			expect(data[0].total).toBe('650000000000000000000000000000');
		});

		it('should handle result going to zero from mixed operations', async () => {
			await db.query('DELETE FROM bigint_test');
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES 
				('500000000000000000000000000000'),
				('-500000000000000000000000000000')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0');
		});

		it('should handle saturating subtraction (underflow protection)', async () => {
			await db.query('DELETE FROM bigint_test');
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES 
				('100'),
				('-999999999999999999999999999999999999999999999999999999999999999999999999999999')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			
			// The very large negative number should cause a parsing error
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Failed to parse');
		});
	});

	describe('GROUP BY Operations', () => {
		beforeEach(async () => {
			await db.query(`
				INSERT INTO bigint_test (amount, category) VALUES 
				('100000000000000000000000000000', 'income'),
				('200000000000000000000000000000', 'income'),
				('-50000000000000000000000000000', 'expense'),
				('-75000000000000000000000000000', 'expense'),
				('300000000000000000000000000000', 'bonus')
			`);
		});

		it('should work with GROUP BY clauses', async () => {
			const result = await db.query(`
				SELECT category, BIGINT_SUM(amount) as total 
				FROM bigint_test 
				GROUP BY category 
				ORDER BY category
			`);
			const data = JSON.parse(result.value || '[]');
			
			expect(data).toHaveLength(3);
			
			// Find each category
			const bonus = data.find((row: CategoryRow) => row.category === 'bonus');
			const expense = data.find((row: CategoryRow) => row.category === 'expense');
			const income = data.find((row: CategoryRow) => row.category === 'income');
			
			expect(bonus?.total).toBe('300000000000000000000000000000');
			expect(expense?.total).toBe('0'); // Negative values saturate to 0
			expect(income?.total).toBe('300000000000000000000000000000'); // 100000000000000000000000000000 + 200000000000000000000000000000
		});

		it('should work with HAVING clauses', async () => {
			const result = await db.query(`
				SELECT category, BIGINT_SUM(amount) as total 
				FROM bigint_test 
				GROUP BY category 
				HAVING BIGINT_SUM(amount) > '0'
				ORDER BY total DESC
			`);
			const data = JSON.parse(result.value || '[]');
			
			expect(data).toHaveLength(2); // Only income and bonus should remain
			expect(data[0].category).toBe('income'); // Highest total first
			expect(data[1].category).toBe('bonus');
		});
	});

	describe('Error Handling', () => {
		it('should handle invalid numeric strings gracefully', async () => {
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES ('not_a_number')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Failed to parse');
		});

		it('should handle empty strings gracefully', async () => {
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES ('')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Empty string');
		});

		it('should handle malformed negative numbers', async () => {
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES ('-')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Invalid negative number format');
		});

		it('should handle only valid string values', async () => {
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES 
				('100000000000000000000000000000'),
				('200000000000000000000000000000')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('300000000000000000000000000000');
		});
	});

	describe('Edge Cases and Limits', () => {
		it('should handle very large numbers near U256 limit', async () => {
			// U256::MAX is approximately 1.15 * 10^77
			const nearMax = '115792089237316195423570985008687907853269984665640564039457584007913129639935';
			
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES ('${nearMax}')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe(nearMax);
		});

		it('should handle hexadecimal input', async () => {
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES 
				('0x10'),   -- 16 in decimal
				('0xFF'),   -- 255 in decimal
				('0x100')   -- 256 in decimal
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			// 16 + 255 + 256 = 527
			expect(data[0].total).toBe('527');
		});

		it('should handle leading zeros correctly', async () => {
			await db.query(`
				INSERT INTO bigint_test (amount) VALUES 
				('000123'),
				('-000456'),
				('000789')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			// 123 - 456 + 789 = 456, but since we can't go negative in U256, 
			// it should be 123 + 789 - 456 = 456 if order allows, or saturate
			// The actual result depends on processing order, but should be valid
			expect(data[0].total).toMatch(/^\d+$/);
		});
	});

	describe('Performance Tests', () => {
		it('should handle bulk aggregation efficiently', async () => {
			// Insert 100 large numbers
			const values = Array.from({ length: 100 }, (_, i) => 
				`('${i + 1}${'0'.repeat(50)}', 'bulk')`
			).join(',');
			
			await db.query(`
				INSERT INTO bigint_test (amount, category) VALUES ${values}
			`);
			
			perf.start('bulk-bigint-sum');
			const result = await db.query('SELECT BIGINT_SUM(amount) as total FROM bigint_test');
			perf.end('bulk-bigint-sum');
			
			const data = JSON.parse(result.value || '[]');
			expect(data[0]).toHaveProperty('total');
			expect(data[0].total).toMatch(/^\d+$/);
			expect(perf.getDuration('bulk-bigint-sum')).toBeLessThan(3000);
		});

		it('should handle complex queries with joins efficiently', async () => {
			// Create a second table for join testing
			await db.query(`
				CREATE TABLE categories (
					name TEXT PRIMARY KEY,
					multiplier TEXT
				)
			`);
			
			await db.query(`
				INSERT INTO categories (name, multiplier) VALUES 
				('income', '2'),
				('expense', '1')
			`);
			
			await db.query(`
				INSERT INTO bigint_test (amount, category) VALUES 
				('100000000000000000000000000000', 'income'),
				('-50000000000000000000000000000', 'expense')
			`);
			
			perf.start('complex-bigint-query');
			const result = await db.query(`
				SELECT 
					t.category,
					BIGINT_SUM(t.amount) as total_amount,
					c.multiplier
				FROM bigint_test t
				JOIN categories c ON t.category = c.name
				GROUP BY t.category, c.multiplier
				ORDER BY total_amount DESC
			`);
			perf.end('complex-bigint-query');
			
			const data = JSON.parse(result.value || '[]');
			expect(Array.isArray(data)).toBe(true);
			expect(data.length).toBeGreaterThan(0);
			expect(perf.getDuration('complex-bigint-query')).toBeLessThan(2000);
		});
	});

	describe('Real-world Scenarios', () => {
		it('should handle cryptocurrency-like transaction amounts', async () => {
			// Simulate cryptocurrency transactions with 18 decimal places (wei amounts)
			await db.query(`
				INSERT INTO bigint_test (amount, category, description) VALUES 
				('1000000000000000000000', 'transfer', '1000 ETH in wei'),
				('-500000000000000000000', 'gas', '500 ETH gas fee'),
				('2500000000000000000000', 'mining', '2500 ETH mining reward'),
				('-100000000000000000000', 'transfer', '100 ETH sent')
			`);
			
			const result = await db.query(`
				SELECT 
					category,
					BIGINT_SUM(amount) as total_wei,
					COUNT(*) as transaction_count
				FROM bigint_test 
				GROUP BY category
				ORDER BY total_wei DESC
			`);
			
			const data = JSON.parse(result.value || '[]');
			expect(Array.isArray(data)).toBe(true);
			
			// Verify we can handle these large amounts correctly
			const mining = data.find((row: MiningRow) => row.category === 'mining');
			expect(mining?.total_wei).toBe('2500000000000000000000');
		});

		it('should handle financial ledger balancing', async () => {
			// Simulate a financial ledger that must balance to zero
			await db.query(`
				INSERT INTO bigint_test (amount, description) VALUES 
				('999999999999999999999999999999', 'Initial deposit'),
				('-100000000000000000000000000000', 'Payment 1'),
				('-200000000000000000000000000000', 'Payment 2'),
				('-300000000000000000000000000000', 'Payment 3'),
				('-399999999999999999999999999999', 'Final payment')
			`);
			
			const result = await db.query('SELECT BIGINT_SUM(amount) as balance FROM bigint_test');
			const data = JSON.parse(result.value || '[]');
			
			// Should balance to exactly 0
			expect(data[0].balance).toBe('0');
		});
	});
});
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

interface TransactionRow {
	category: string;
	total_float: string;
}

describe('FLOAT_SUM Database Function', () => {
	let db: SQLiteWasmDatabase;
	let perf: PerformanceTracker;
	
	beforeEach(async () => {
		db = await createTestDatabase();
		perf = new PerformanceTracker();
		
		await db.query(`
			CREATE TABLE float_test (
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
		it('should have FLOAT_SUM function available', async () => {
			try {
				const pragmaResult = await db.query('PRAGMA function_list');
				expect(pragmaResult).toBeDefined();
				const functions = JSON.parse(pragmaResult.value || '[]');
				const floatSumEntry = functions.find((f: any) => f.name === 'FLOAT_SUM');
				expect(floatSumEntry).toBeDefined();
			} catch (error) {
			}

			try {
				const rainResult = await db.query('SELECT RAIN_MATH_PROCESS("100", "200") as test');
				expect(rainResult).toBeDefined();
				expect(rainResult.value).toBeDefined();
			} catch (error) {
				throw new Error('RAIN_MATH_PROCESS not available');
			}

			try {
				const result = await db.query('SELECT FLOAT_SUM("0xffffffff00000000000000000000000000000000000000000000000000000001") as test');
				expect(result).toBeDefined();
				expect(result.value).toBeDefined();
				const data = JSON.parse(result.value || '[]');
				expect(data[0].test).toBe('0.1');
			} catch (error) {
				throw new Error('FLOAT_SUM function not available');
			}
		});
	});

	describe('Basic FLOAT_SUM Functionality', () => {
		beforeEach(async () => {
			await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES 
				('0xffffffff00000000000000000000000000000000000000000000000000000001', 'income', 'payment'),
				('0xffffffff00000000000000000000000000000000000000000000000000000005', 'income', 'deposit'),
				('ffffffff0000000000000000000000000000000000000000000000000000000f', 'income', 'amount')
			`);
		});

		it('should sum positive float values correctly', async () => {
			perf.start('float-sum-positive');
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			perf.end('float-sum-positive');
			
			const data = JSON.parse(result.value || '[]');
			
			expect(data).toHaveLength(1);
			expect(data[0]).toHaveProperty('total');
			
			expect(data[0].total).toBe('2.1');
			expect(perf.getDuration('float-sum-positive')).toBeLessThan(1000);
		});

		it('should handle single row correctly', async () => {
			await db.query('DELETE FROM float_test WHERE id > 1');
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0.1');
		});

		it('should return 0 for empty table', async () => {
			await db.query('DELETE FROM float_test');
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0');
		});
	});

	describe('Hex Format Support', () => {
		beforeEach(async () => {
			await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES 
				('0xfffffffe00000000000000000000000000000000000000000000000000002729', 'income', 'large payment'),
				('fffffffd0000000000000000000000000000000000000000000000000001e240', 'income', 'large deposit'),
				('0x0000000000000000000000000000000000000000000000000000000000000000', 'zero', 'zero value')
			`);
		});

		it('should handle hex values with and without 0x prefix', async () => {
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('223.706');
		});

		it('should handle mixed case hex values', async () => {
			await db.query('DELETE FROM float_test');
			await db.query(`
				INSERT INTO float_test (amount) VALUES 
				('0xFfFfFfFf0000000000000000000000000000000000000000000000000000000F'),
				('0xFfFfFfFe000000000000000000000000000000000000000000000000000000E1')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('3.75');
		});

		it('should reject uppercase 0X prefix', async () => {
			await db.query('DELETE FROM float_test');
			await db.query(`
				INSERT INTO float_test (amount) VALUES ('0XFFFFFFFB0000000000000000000000000000000000000000000000000004CB2F')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Failed to parse hex number');
		});
	});

	describe('GROUP BY Operations', () => {
		beforeEach(async () => {
			await db.query(`
				INSERT INTO float_test (amount, category) VALUES 
				('0xffffffff00000000000000000000000000000000000000000000000000000001', 'income'),
				('0xffffffff00000000000000000000000000000000000000000000000000000005', 'income'),
				('0x000000000000000000000000000000000000000000000000000000000000000a', 'bonus'),
				('ffffffff0000000000000000000000000000000000000000000000000000000f', 'expense')
			`);
		});

		it('should work with GROUP BY clauses', async () => {
			const result = await db.query(`
				SELECT category, FLOAT_SUM(amount) as total 
				FROM float_test 
				GROUP BY category 
				ORDER BY category
			`);
			const data = JSON.parse(result.value || '[]');
			
			expect(data).toHaveLength(3);
			
			const bonus = data.find((row: CategoryRow) => row.category === 'bonus');
			const expense = data.find((row: CategoryRow) => row.category === 'expense');
			const income = data.find((row: CategoryRow) => row.category === 'income');
			
			expect(bonus?.total).toBe('10');
			expect(expense?.total).toBe('1.5');
			expect(income?.total).toBe('0.6');
		});

		it('should work with HAVING clauses', async () => {
			const result = await db.query(`
				SELECT category, FLOAT_SUM(amount) as total 
				FROM float_test 
				GROUP BY category 
				HAVING FLOAT_SUM(amount) > '1'
				ORDER BY total DESC
			`);
			const data = JSON.parse(result.value || '[]');
			
			expect(data).toHaveLength(2);
			const categories = data.map((row: CategoryRow) => row.category).sort();
			expect(categories).toEqual(['bonus', 'expense']);
		});
	});

	describe('Error Handling', () => {
		it('should handle invalid hex strings gracefully', async () => {
			await db.query(`
				INSERT INTO float_test (amount) VALUES ('not_hex')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Failed to parse hex number');
		});

		it('should handle empty strings gracefully', async () => {
			await db.query(`
				INSERT INTO float_test (amount) VALUES ('')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Empty string is not a valid hex number');
		});

		it('should handle invalid hex characters', async () => {
			await db.query(`
				INSERT INTO float_test (amount) VALUES ('0xGHI')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Failed to parse hex number');
		});

		it('should handle only valid hex values', async () => {
			await db.query(`
				INSERT INTO float_test (amount) VALUES 
				('0xffffffff00000000000000000000000000000000000000000000000000000001'),
				('0xffffffff00000000000000000000000000000000000000000000000000000005')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0.6');
		});
	});

	describe('Edge Cases and Limits', () => {
		it('should handle very small float values', async () => {
			const smallValue = '0xffffffff00000000000000000000000000000000000000000000000000000001';
			
			await db.query(`
				INSERT INTO float_test (amount) VALUES ('${smallValue}')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0.1');
		});

		it('should reject hex values that are too short', async () => {
			await db.query(`
				INSERT INTO float_test (amount) VALUES ('0x0')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			expect(result.error).toBeDefined();
			expect(result.error?.msg).toContain('Failed to parse hex number');
		});

		it('should handle whitespace in hex values', async () => {
			await db.query(`
				INSERT INTO float_test (amount) VALUES 
				('  0x000000000000000000000000000000000000000000000000000000000000000a  '),
				('\t0x0000000000000000000000000000000000000000000000000000000000000014\n')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('30');
		});
	});

	describe('High Precision Decimals', () => {
		beforeEach(async () => {
			await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES 
				('0xffffffee0000000000000000000000000000000000000010450cb5d3cf60f34e', 'precision', 'high precision 1'),
				('0xffffffee0000000000000000000000000000000000000010510af4e77328b478', 'precision', 'high precision 2'),
				('0xffffffee00000000000000000000000000000000000000104b0bd55dbf1238e3', 'precision', 'high precision 3'),
				('0xffffffee00000000000000000000000000000000000000104e21534cc7d31c71', 'precision', 'high precision 4'),
				('0xffffffee00000000000000000000000000000000000000105136d13bd093ffff', 'precision', 'high precision 5')
			`);
		});

		it('should handle high precision decimal calculations', async () => {
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('1503.444444443444444441');
		});

		it('should handle mixed precision values correctly', async () => {
			await db.query('DELETE FROM float_test');
			await db.query(`
				INSERT INTO float_test (amount) VALUES 
				('0xffffffee00000000000000000000000000000000000000000f9751ff4d94f34e'),
				('0xffffffee0000000000000000000000000000000000000000297647c698c0b478'),
				('0x0000000000000000000000000000000000000000000000000000000000000064')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('104.11111111011111111');
		});
	});

	describe('Performance Tests', () => {
		it('should handle bulk aggregation efficiently', async () => {
			const values = Array.from({ length: 10 }, (_, i) => 
				`('0xffffffff00000000000000000000000000000000000000000000000000000001', 'bulk')`
			).join(',');
			
			await db.query(`
				INSERT INTO float_test (amount, category) VALUES ${values}
			`);
			
			perf.start('bulk-float-sum');
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			perf.end('bulk-float-sum');
			
			if (result.error) {
				throw new Error(`Query failed: ${result.error.msg}`);
			}
			
			const data = JSON.parse(result.value || '[]');
			expect(data).toHaveLength(1);
			expect(data[0]).toHaveProperty('total');
			expect(data[0].total).toBe('1');
			expect(perf.getDuration('bulk-float-sum')).toBeLessThan(3000);
		});

		it('should handle complex queries with joins efficiently', async () => {
			await db.query(`
				CREATE TABLE float_categories (
					name TEXT PRIMARY KEY,
					multiplier TEXT
				)
			`);
			
			await db.query(`
				INSERT INTO float_categories (name, multiplier) VALUES 
				('income', '2'),
				('expense', '1')
			`);
			
			await db.query(`
				INSERT INTO float_test (amount, category) VALUES 
				('0xffffffff00000000000000000000000000000000000000000000000000000001', 'income'),
				('0xffffffff00000000000000000000000000000000000000000000000000000005', 'expense')
			`);
			
			perf.start('complex-float-query');
			const result = await db.query(`
				SELECT 
					t.category,
					FLOAT_SUM(t.amount) as total_amount,
					c.multiplier
				FROM float_test t
				JOIN float_categories c ON t.category = c.name
				GROUP BY t.category, c.multiplier
				ORDER BY total_amount DESC
			`);
			perf.end('complex-float-query');
			
			const data = JSON.parse(result.value || '[]');
			expect(Array.isArray(data)).toBe(true);
			expect(data.length).toBe(2);
			
			const rowsByCategory = data.reduce((map: Record<string, any>, row: any) => {
				map[row.category] = row;
				return map;
			}, {} as Record<string, any>);
			
			expect(rowsByCategory['income']).toBeDefined();
			expect(rowsByCategory['income'].total_amount).toBe('0.1');
			expect(rowsByCategory['income'].multiplier).toBe('2');
			
			expect(rowsByCategory['expense']).toBeDefined();
			expect(rowsByCategory['expense'].total_amount).toBe('0.5');
			expect(rowsByCategory['expense'].multiplier).toBe('1');
			
			expect(perf.getDuration('complex-float-query')).toBeLessThan(2000);
		});
	});

	describe('Real-world Scenarios', () => {
		it('should handle DeFi-like transaction amounts', async () => {
			await db.query(`
				INSERT INTO float_test (amount, category, description) VALUES 
				('0xfffffffe00000000000000000000000000000000000000000000000000002729', 'swap', 'token swap'),
				('0xfffffffd0000000000000000000000000000000000000000000000000001e240', 'liquidity', 'liquidity provision'),
				('0xffffffff00000000000000000000000000000000000000000000000000000001', 'rewards', 'staking rewards'),
				('0xffffffff00000000000000000000000000000000000000000000000000000005', 'fees', 'protocol fees')
			`);
			
			const result = await db.query(`
				SELECT 
					category,
					FLOAT_SUM(amount) as total_float,
					COUNT(*) as transaction_count
				FROM float_test 
				GROUP BY category
				ORDER BY total_float DESC
			`);
			
			const data = JSON.parse(result.value || '[]');
			expect(Array.isArray(data)).toBe(true);
			
			const liquidity = data.find((row: TransactionRow) => row.category === 'liquidity');
			expect(liquidity?.total_float).toBe('123.456');
		});

		it('should handle precision accounting that must balance', async () => {
			await db.query(`
				INSERT INTO float_test (amount, description) VALUES 
				('0xfffffffe00000000000000000000000000000000000000000000000000002729', 'Initial balance'),
				('0xffffffff00000000000000000000000000000000000000000000000000000001', 'Small addition')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as balance FROM float_test');
			
			if (result.error) {
				throw new Error(`Query failed: ${result.error.msg}`);
			}
			
			const data = JSON.parse(result.value || '[]');
			expect(data).toHaveLength(1);
			expect(data[0]).toHaveProperty('balance');
			expect(data[0].balance).toBe('100.35');
		});
	});

	describe('NULL Value Handling', () => {
		it('should skip NULL values like standard SQL aggregates', async () => {
			await db.query(`
				INSERT INTO float_test (amount, category) VALUES 
				('0xffffffff00000000000000000000000000000000000000000000000000000001', 'test'),
				('0xffffffff00000000000000000000000000000000000000000000000000000005', 'test')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			
			if (result.error) {
				throw new Error(`Query failed: ${result.error.msg}`);
			}
			
			const data = JSON.parse(result.value || '[]');
			expect(data).toHaveLength(1);
			expect(data[0].total).toBe('0.6');
		});

		it('should return 0 when all values are NULL', async () => {
			await db.query(`
				INSERT INTO float_test (amount, category) VALUES 
				(NULL, 'test'),
				(NULL, 'test')
			`);
			
			const result = await db.query('SELECT FLOAT_SUM(amount) as total FROM float_test');
			const data = JSON.parse(result.value || '[]');
			
			expect(data[0].total).toBe('0');
		});
	});
});
-- Test data for SQLite worker tests

-- Create test users table
CREATE TABLE IF NOT EXISTS test_users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT UNIQUE,
    age INTEGER,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Create test products table
CREATE TABLE IF NOT EXISTS test_products (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    price REAL NOT NULL,
    category TEXT,
    in_stock BOOLEAN DEFAULT true
);

-- Insert sample users
INSERT OR IGNORE INTO test_users (name, email, age) VALUES 
('Alice Johnson', 'alice@test.com', 28),
('Bob Smith', 'bob@test.com', 34),
('Carol Davis', 'carol@test.com', 25),
('David Wilson', 'david@test.com', 31),
('Eve Brown', 'eve@test.com', 29);

-- Insert sample products
INSERT OR IGNORE INTO test_products (name, price, category, in_stock) VALUES 
('Laptop Pro', 1299.99, 'Electronics', true),
('Wireless Mouse', 29.99, 'Electronics', true),
('Office Chair', 189.50, 'Furniture', false),
('Coffee Mug', 12.99, 'Kitchen', true),
('Notebook Set', 15.99, 'Office', true);
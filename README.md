# SQLite Worker - Browser-Native Database with OPFS

A production-ready WebAssembly SQLite implementation that runs entirely in the browser using web workers, with persistent storage via Origin Private File System (OPFS) and multi-worker coordination through leader election.

## 🚀 Features

- **Real SQLite Database** - Uses `sqlite-wasm-rs` FFI bindings for authentic SQLite operations
- **OPFS Persistent Storage** - Data survives browser refreshes and sessions
- **Multi-Worker Architecture** - Leader election with BroadcastChannel coordination
- **Self-Contained Workers** - Dynamic WASM loading with absolute URLs
- **Type-Safe Results** - JSON-formatted query results with proper column typing
- **Zero External Dependencies** - Complete WASM module embedded in workers
- **ES6 Module Support** - Modern import/export syntax for easy integration

## 🏗️ Architecture

### Core Components

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Main Thread   │───▶│   Web Worker     │───▶│  SQLite OPFS    │
│                 │    │                  │    │                 │
│ DatabaseConnection   │  • Leader Election    │  • Persistent   │
│ • Query API     │    │  • SQL Execution │    │  • Transactions │
│ • Promise-based │    │  • Error Handling│    │  • ACID         │
└─────────────────┘    └──────────────────┘    └─────────────────┘
         │                       │
         └───────────────────────┼─────────────────────────────────┐
                                 │                                 │
                    ┌──────────────────┐              ┌──────────────────┐
                    │  BroadcastChannel │              │   Web Locks API  │
                    │                  │              │                  │
                    │ • Query Routing  │              │ • Leader Election│
                    │ • Multi-Worker   │              │ • Exclusive Lock │
                    │ • Message Passing│              │ • Failover       │
                    └──────────────────┘              └──────────────────┘
```

### Worker Communication Flow

1. **Main Thread** creates `DatabaseConnection` 
2. **Worker Creation** - Generates self-contained worker with embedded WASM
3. **WASM Initialization** - Dynamic import and worker_main() execution
4. **Leader Election** - Uses Web Locks API for exclusive database access
5. **OPFS Setup** - Installs sahpool VFS for persistent storage
6. **Query Execution** - Direct SQLite FFI calls with prepared statements
7. **Result Formatting** - JSON serialization with proper data types

## 🛠️ Implementation Details

### SQLite Integration

```rust
// Real SQLite database using sqlite-wasm-rs FFI
use sqlite_wasm_rs::{self as ffi, sahpool_vfs::install as install_opfs_vfs};

struct SQLiteDatabase {
    db: *mut ffi::sqlite3,
}

impl SQLiteDatabase {
    async fn initialize_opfs() -> Result<Self, JsValue> {
        // Install OPFS VFS and set as default
        install_opfs_vfs(None, true).await?;
        
        // Open database with OPFS persistence
        let db_name = CString::new("opfs-sahpool:worker.db")?;
        let mut db = std::ptr::null_mut();
        
        let ret = unsafe {
            ffi::sqlite3_open_v2(
                db_name.as_ptr(),
                &mut db,
                ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
                std::ptr::null()
            )
        };
        
        Ok(SQLiteDatabase { db })
    }
}
```

### Worker Architecture

```rust
// Entry point for the worker - called from the blob
#[wasm_bindgen]
pub fn worker_main() -> Result<(), JsValue> {
    let state = Rc::new(WorkerState::new()?);
    
    // Setup BroadcastChannel listener for multi-worker coordination
    state.setup_channel_listener();
    
    // Attempt leadership using Web Locks API
    spawn_local(async move {
        state_clone.attempt_leadership().await;
    });
    
    // Setup message handler from main thread
    setup_main_thread_message_handler();
    
    Ok(())
}
```

### Query Processing

```rust
async fn exec(&self, sql: &str) -> Result<String, String> {
    let sql_cstr = CString::new(sql)?;
    let mut stmt = std::ptr::null_mut();
    
    // Prepare statement
    let ret = unsafe {
        ffi::sqlite3_prepare_v2(self.db, sql_cstr.as_ptr(), -1, &mut stmt, std::ptr::null_mut())
    };
    
    // Execute and collect results
    let mut results = Vec::new();
    loop {
        match unsafe { ffi::sqlite3_step(stmt) } {
            ffi::SQLITE_ROW => {
                // Extract row data with proper typing
                let row_data = extract_typed_row_data(stmt)?;
                results.push(row_data);
            }
            ffi::SQLITE_DONE => break,
            _ => return Err("Query execution failed".into()),
        }
    }
    
    // Return JSON-formatted results
    serde_json::to_string_pretty(&results)
}
```

## 📦 Dependencies

```toml
[dependencies]
# Core WebAssembly bindings
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
js-sys = "0.3"
web-sys = { version = "0.3", features = [
    "Blob", "BlobPropertyBag", "Url", "Worker", "MessageEvent",
    "BroadcastChannel", "DedicatedWorkerGlobalScope"
]}

# SQLite WebAssembly implementation
sqlite-wasm-rs = { version = "0.3", default-features = false, features = ["precompiled"] }

# Serialization and utilities
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde-wasm-bindgen = "0.6"
uuid = { version = "1.0", features = ["v4", "js"] }
console_error_panic_hook = "0.1"
```

## 🚀 Build Process

### Development Build
```bash
# Basic compilation check
cargo build

# Generate WASM package for browser
wasm-pack build --target web
```

### Production Build
```bash
# Build with embedded WASM support
cargo build --release --features embed_wasm

# Generate optimized WASM package
wasm-pack build --target web --release
```

### Build Features
- **`embed_wasm`** - Enables compile-time WASM embedding (currently unused due to circular dependency)
- **`precompiled`** - Uses pre-built SQLite WASM instead of compiling from source

## 🖥️ Usage

### Basic Setup

```html
<!DOCTYPE html>
<html>
<head>
    <title>SQLite Worker Demo</title>
</head>
<body>
    <script type="module">
        import init, { DatabaseConnection } from './pkg/sqlite_worker.js';
        
        async function initDatabase() {
            // Initialize WASM module
            await init();
            
            // Create database connection (spawns worker)
            const db = new DatabaseConnection();
            
            // Wait for worker initialization
            await new Promise(resolve => setTimeout(resolve, 1000));
            
            return db;
        }
        
        async function runQueries() {
            const db = await initDatabase();
            
            // Create table
            await db.query(`
                CREATE TABLE users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    email TEXT UNIQUE
                )
            `);
            
            // Insert data
            await db.query("INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')");
            
            // Query data
            const result = await db.query("SELECT * FROM users");
            console.log('Query results:', result);
        }
        
        runQueries();
    </script>
</body>
</html>
```

### API Reference

#### `DatabaseConnection`

##### Constructor
```javascript
const db = new DatabaseConnection();
```
Creates a new database connection and spawns a web worker with embedded SQLite.

##### Methods

###### `query(sql: string): Promise<string>`
Executes a SQL query and returns results as JSON string.

```javascript
// DDL operations
await db.query("CREATE TABLE posts (id INTEGER PRIMARY KEY, title TEXT)");

// DML operations  
await db.query("INSERT INTO posts (title) VALUES ('Hello World')");

// Queries return JSON-formatted results
const results = await db.query("SELECT * FROM posts");
console.log(JSON.parse(results));
// [{"id": 1, "title": "Hello World"}]
```

## 🔧 Server Setup

### Development Server (Bun)

```javascript
// server.js
const server = Bun.serve({
  port: 3000,
  fetch(req) {
    const url = new URL(req.url);
    let filePath = url.pathname === '/' ? '/test.html' : url.pathname;
    
    const file = Bun.file(`.${filePath}`);
    const headers = new Headers();
    
    // Set appropriate MIME types
    if (filePath.endsWith('.js')) headers.set('Content-Type', 'application/javascript');
    if (filePath.endsWith('.wasm')) headers.set('Content-Type', 'application/wasm');
    if (filePath.endsWith('.html')) headers.set('Content-Type', 'text/html');
    
    // Enable CORS for all requests
    headers.set('Access-Control-Allow-Origin', '*');
    
    return new Response(file, { headers });
  },
});

console.log(`Server running at http://localhost:${server.port}`);
```

Start server:
```bash
bun server.js
```

## 🎯 Features Deep Dive

### Persistent Storage (OPFS)
- Uses Origin Private File System for true persistence
- Data survives browser refreshes, tab closures, and system restarts
- Provides native file system performance
- Isolated per origin for security

### Multi-Worker Coordination
- **Leader Election** - One worker manages the database, others proxy queries
- **BroadcastChannel** - Inter-worker communication for query routing
- **Web Locks API** - Exclusive database access with automatic failover
- **Query Timeout** - Handles unresponsive leader scenarios

### Error Handling
- **SQLite Error Mapping** - Native SQLite error codes and messages
- **Connection Recovery** - Automatic reconnection on worker failure
- **Transaction Safety** - ACID compliance through SQLite transactions
- **Graceful Degradation** - Fallback mechanisms for unsupported browsers

### Performance Optimizations
- **Prepared Statements** - Compiled queries for repeated execution
- **Typed Results** - Efficient JSON serialization with proper data types
- **Memory Management** - Automatic cleanup of SQLite resources
- **WASM Optimization** - Size-optimized builds with `wasm-opt`

## 🧪 Testing

### Interactive Test Page
The included `test.html` provides a comprehensive testing interface:

- **Database Initialization** - Test worker spawning and OPFS setup
- **Schema Operations** - CREATE TABLE, ALTER TABLE, DROP TABLE
- **Data Manipulation** - INSERT, UPDATE, DELETE with constraint handling
- **Query Testing** - SELECT with various conditions and JOINs
- **Custom SQL** - Direct SQL execution with result display
- **Error Scenarios** - Constraint violations, syntax errors, type mismatches

### Test Commands
```bash
# Start development server
bun server.js

# Open browser
open http://localhost:3000
```

## 🔍 Troubleshooting

### Common Issues

#### WASM Loading Errors
```
Error: CompileError: wasm validation error: at offset 0: failed to match magic number
```
**Solution:** Ensure you're building with the correct target and the WASM file is properly generated.

#### Worker Import Errors
```
TypeError: Error resolving module specifier "./pkg/sqlite_worker.js"
```
**Solution:** Worker uses absolute URLs. Ensure proper server setup with correct MIME types.

#### OPFS Support
```
Failed to install OPFS VFS
```
**Solution:** OPFS requires HTTPS in production. Use `localhost` for development.

#### Unique Constraint Violations
```
UNIQUE constraint failed: users.email
```
**Solution:** Use `INSERT OR IGNORE` or `INSERT OR REPLACE` for duplicate handling.

### Debug Mode
Enable verbose logging by setting:
```javascript
// In browser console
localStorage.setItem('sqlite-worker-debug', 'true');
```

## 📄 License

This project demonstrates advanced WebAssembly and SQLite integration techniques. The implementation showcases:

- **Modern Web APIs** - OPFS, BroadcastChannel, Web Locks, Web Workers
- **Rust-WASM Integration** - FFI bindings, memory management, async operations  
- **Database Architecture** - Leader election, query routing, transaction handling
- **Performance Engineering** - Optimized builds, efficient serialization, resource cleanup

Built with ❤️ using Rust, WebAssembly, and modern web standards.
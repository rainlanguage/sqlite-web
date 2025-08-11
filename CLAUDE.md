# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a **SQLite Worker** project - a browser-native WebAssembly SQLite implementation with persistent storage via OPFS (Origin Private File System) and multi-worker coordination. The project demonstrates advanced WebAssembly, SQLite FFI, and modern web APIs integration.

### Architecture
- **Rust WASM Core**: SQLite FFI bindings compiled to WebAssembly
- **Self-Contained Workers**: Dynamic WASM loading with embedded code generation
- **Multi-Worker Coordination**: Leader election using Web Locks API + BroadcastChannel
- **OPFS Persistence**: True browser-native persistent storage
- **Svelte Integration**: Example frontend demonstrating the SQLite worker

## Development Commands

### Rust WASM Development
```bash
# Build and check Rust code
cargo build

# Generate WASM package for browser
wasm-pack build --target web

# Generate WASM with embedded worker (production)
./bundle.sh

# Generate optimized release WASM
wasm-pack build --target web --release
```

### Svelte Frontend Development
```bash
cd svelte-test

# Install dependencies
bun install  # or npm install

# Start development server
bun run dev  # or npm run dev

# Build for production
bun run build  # or npm run build

# Type check
bun run check  # or npm run check
```

### Testing and Development Server
```bash
# Start simple development server (serves static files)
bun server.js

# Test standalone HTML page
open http://localhost:3000

# Test Svelte integration
cd svelte-test && bun run dev
open http://localhost:5173
```

## Code Architecture

### Core Rust Modules (src/)
- **`lib.rs`** (152 lines): Main API - DatabaseConnection struct, worker creation, promise handling
- **`worker.rs`**: Worker entry point, initializes WorkerState and handles main thread messages
- **`database.rs`**: SQLite FFI implementation with OPFS integration, query execution
- **`coordination.rs`**: Worker coordination, leader election via Web Locks API, BroadcastChannel routing
- **`messages.rs`**: Message type definitions for all communication channels
- **`worker_template.rs`**: JavaScript code generation for self-contained workers

### Key Design Patterns
1. **Worker Coordination**: Only one worker (leader) accesses SQLite directly, others proxy through BroadcastChannel
2. **Self-Contained Workers**: Generated workers embed all WASM code as base64 for zero external dependencies
3. **OPFS Integration**: Uses `sqlite-wasm-rs` with sahpool VFS for persistent storage
4. **Promise-Based API**: Main thread queries return Promises, callbacks stored in pending_queries

### Dependencies & Features
- **sqlite-wasm-rs**: SQLite FFI bindings with OPFS support (`precompiled` feature)
- **wasm-bindgen ecosystem**: JS interop, web-sys for Web APIs
- **serde**: Message serialization between main thread and workers
- **uuid**: Unique IDs for worker coordination and query tracking

## Build System

### WASM Build Process
1. `cargo build` - Basic Rust compilation check
2. `wasm-pack build --target web` - Generate WASM + JS bindings for browser
3. `./bundle.sh` - Create self-contained worker with embedded WASM (base64)

### Bundle Script (`bundle.sh`)
- Builds WASM with `wasm-pack`
- Converts WASM to base64 
- Generates `embedded_worker.js` with WASM data + JS glue code embedded
- Creates fully self-contained worker blob (no external file dependencies)

### Development vs Production
- **Development**: Use `wasm-pack build --target web` + `bun server.js`
- **Production**: Use `./bundle.sh` for self-contained workers + proper HTTPS server

## Integration Patterns

### Basic Usage (Vanilla JS)
```javascript
import init, { DatabaseConnection } from './pkg/sqlite_worker.js';

await init();
const db = new DatabaseConnection();
const result = await db.query("SELECT * FROM users");
```

### SvelteKit Integration
- Import from local package: `import init, { DatabaseConnection } from 'sqlite-worker'`
- Initialize in `onMount` with browser check
- Handle async initialization states
- Parse JSON results from queries

## Development Workflow

### Making Changes to Rust Code
1. Edit Rust sources in `src/`
2. Run `cargo build` to check compilation
3. Run `wasm-pack build --target web` to generate WASM
4. Test with `bun server.js` and `test.html`

### Making Changes to Frontend
1. Edit Svelte components in `svelte-test/src/`
2. Development server auto-reloads: `bun run dev`
3. Type check with `bun run check`

### Testing Multi-Worker Coordination
- Open multiple browser tabs to test leader election
- Use browser dev tools to inspect BroadcastChannel messages
- Check Web Locks API in Application tab

## Troubleshooting

### WASM Loading Issues
- Ensure proper MIME types: `.wasm` → `application/wasm`, `.js` → `application/javascript`
- Use HTTPS in production (required for OPFS)
- Check browser console for worker initialization errors

### OPFS Issues  
- OPFS requires secure context (HTTPS or localhost)
- Check if browser supports OPFS: `navigator.storage?.getDirectory`
- Look for "Failed to install OPFS VFS" errors

### Multi-Worker Issues
- Check BroadcastChannel support
- Verify Web Locks API availability
- Look for leader election timeouts in console

## Key Files to Know

- `src/lib.rs`: Main API entry point
- `bundle.sh`: Production build script
- `server.js`: Development server
- `test.html`: Standalone test page
- `svelte-test/src/routes/+page.svelte`: Full integration example
- `pkg/`: Generated WASM output directory (gitignored except README)
#!/bin/bash
set -e

echo "🔨 Building SQLite Worker with embedded WASM..."

# Clean up pkg directory first for a clean slate
echo "🧹 Cleaning up pkg directory..."
rm -rf pkg/*.tgz 2>/dev/null || true

# Clear embedded_worker.js file contents first
echo "🧹 Clearing embedded_worker.js..."
echo "" > src/embedded_worker.js

# Build with web target for better bundler compatibility
echo "📦 Building WASM with web target..."
wasm-pack build --target web --out-dir pkg

# Check if build succeeded
if [ ! -f "pkg/sqlite_worker_bg.wasm" ] || [ ! -f "pkg/sqlite_worker.js" ]; then
    echo "❌ Build failed - missing generated files"
    exit 1
fi

echo "📖 Processing WASM file..."
# Create base64 file (no line wrapping) 
base64 < pkg/sqlite_worker_bg.wasm | tr -d '\n' > pkg/sqlite_worker_bg.wasm.b64

echo "🔧 Generating embedded worker template..."

# Create the embedded worker with fetch interceptor
cat > src/embedded_worker.js << 'EOF'
(function(){
  // Base64 decoder utility - works in both Node.js and browser
  self.__b64ToU8 = function(b64) {
    const decode = typeof atob === 'function' ? atob : (b) => Buffer.from(b, 'base64').toString('binary');
    const s = decode(b64);
    const u8 = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) u8[i] = s.charCodeAt(i);
    return u8;
  };
  
  // Fetch interceptor - intercepts WASM file requests
  self.fetch = (function(originalFetch) {
    return function(resource, init) {
      try {
        const resourceStr = typeof resource === 'string' ? resource : resource.toString();
        if (resourceStr.includes('sqlite_worker_bg.wasm') || resourceStr === './sqlite_worker_bg.wasm') {
          const bytes = self.__b64ToU8(self.__WASM_B64_MAP['sqlite_worker_bg.wasm']);
          return Promise.resolve(new Response(bytes, { 
            headers: { 'Content-Type': 'application/wasm' } 
          }));
        }
      } catch (e) {
        console.warn('Fetch interceptor error:', e);
      }
      return originalFetch.call(this, resource, init);
    };
  })(self.fetch || (() => Promise.reject(new Error('fetch not available'))));
  
  // WASM base64 data map
  self.__WASM_B64_MAP = {'sqlite_worker_bg.wasm': '__WASM_B64_SQLITE__'};
  
  // Embedded wasm-bindgen glue code
JS_GLUE_PLACEHOLDER
  
  // Initialize the worker after everything is set up
  // For web target, wasm_bindgen is a function, not an object
  console.log('[Worker] Initializing WASM...');
  wasm_bindgen('./sqlite_worker_bg.wasm').then(function(wasm) {
    console.log('[Worker] WASM loaded, starting worker_main...');
    if (typeof wasm.worker_main === 'function') {
      wasm.worker_main();
      console.log('[Worker] SQLite worker initialized successfully');
      self.postMessage({type: 'worker-ready'});
    } else {
      throw new Error('worker_main function not found');
    }
  }).catch(function(error) {
    console.error('[Worker] Initialization failed:', error);
    self.postMessage({
      type: 'worker-error', 
      error: error.toString()
    });
  });
})();
EOF

echo "🔄 Assembling final worker..."

# Create the final embedded worker by combining template + JS glue + base64 substitution
{
  # Start with the template (everything before JS_GLUE_PLACEHOLDER)
  sed '/JS_GLUE_PLACEHOLDER/,$d' src/embedded_worker.js
  
  # Add the JS glue code (convert exports to regular variables for worker context)
  sed 's/^export function /function /; s/^export class /class /; s/^export { initSync };/self.initSync = initSync;/; s/^export default __wbg_init;/self.wasm_bindgen = __wbg_init;/; s/import\.meta\.url/self.location.href/g' pkg/sqlite_worker.js
  
  # Add the rest of the template (everything after JS_GLUE_PLACEHOLDER)
  sed '1,/JS_GLUE_PLACEHOLDER/d' src/embedded_worker.js
} | awk 'BEGIN{getline b64<"pkg/sqlite_worker_bg.wasm.b64"} {gsub(/__WASM_B64_SQLITE__/, b64)}1' > src/embedded_worker.js.final

# Replace the original with the final version
mv src/embedded_worker.js.final src/embedded_worker.js

echo "✅ Bundle complete! Generated src/embedded_worker.js"
echo "📊 Embedded WASM size: $(wc -c < pkg/sqlite_worker_bg.wasm.b64) base64 characters"
echo "📊 JS glue code lines: $(wc -l < pkg/sqlite_worker.js)"

echo ""
echo "🚀 Your SQLite worker is now fully self-contained!"
echo "   No external dependencies required - everything is embedded in the worker blob."

# Run wasm-pack again to use the populated embedded_worker.js file
echo "📦 Running wasm-pack build again with populated embedded_worker.js..."
wasm-pack build --target web --out-dir pkg

# Package the result
echo "📦 Packaging with npm pack..."
cd pkg
npm pack
cd ..

# Update Svelte integration with fresh package
echo "🔄 Updating Svelte integration..."
cd svelte-test
bun remove sqlite-worker
rm -rf node_modules
bun add ../pkg/sqlite-worker-*.tgz
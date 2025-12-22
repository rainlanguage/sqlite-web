#!/bin/bash
set -e

echo "🔨 Building SQLite Worker with workspace architecture..."

# Clean up pkg directory first for a clean slate
echo "🧹 Cleaning up pkg directory..."
rm -rf pkg/*.tgz 2>/dev/null || true

# Clear embedded_worker.js file contents first
echo "🧹 Clearing embedded_worker.js..."
echo "" > crates/sqlite-web/src/embedded_worker.js

echo "📦 Step 1: Building core package with web target..."
cd crates/sqlite-web-core
wasm-pack build --target web --out-dir ../../pkg
cd ../..

# Check if build succeeded
if [ ! -f "pkg/sqlite_web_core_bg.wasm" ] || [ ! -f "pkg/sqlite_web_core.js" ]; then
    echo "❌ Core build failed - missing generated files"
    exit 1
fi

echo "📖 Processing core WASM file..."
# Create base64 file (no line wrapping) 
base64 < pkg/sqlite_web_core_bg.wasm | tr -d '\n' > pkg/sqlite_web_core_bg.wasm.b64

echo "🔧 Generating embedded worker template..."

# Create the embedded worker with fetch interceptor
cat > crates/sqlite-web/src/embedded_worker.js << 'EOF'
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
        if (resourceStr.includes('sqlite_web_core_bg.wasm') || resourceStr === './sqlite_web_core_bg.wasm') {
          const bytes = self.__b64ToU8(self.__WASM_B64_MAP['sqlite_web_core_bg.wasm']);
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
  self.__WASM_B64_MAP = {'sqlite_web_core_bg.wasm': '__WASM_B64_CORE__'};
  
  // Embedded wasm-bindgen glue code
JS_GLUE_PLACEHOLDER
  
  // Initialize the worker after everything is set up
  // For web target, wasm_bindgen is a function, not an object
  console.log('[Worker] Initializing core WASM...');
  wasm_bindgen('./sqlite_web_core_bg.wasm').then(function(wasm) {
    console.log('[Worker] Core WASM loaded, starting worker_main...');
    if (typeof wasm.worker_main === 'function') {
      wasm.worker_main();
      console.log('[Worker] SQLite worker initialized successfully');
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
  sed '/JS_GLUE_PLACEHOLDER/,$d' crates/sqlite-web/src/embedded_worker.js
  
  # Add the JS glue code (convert exports to regular variables for worker context)
  sed 's/^export function /function /; s/^export class /class /; s/^export { initSync };/self.initSync = initSync;/; s/^export default __wbg_init;/self.wasm_bindgen = __wbg_init;/; s/import\.meta\.url/self.location.href/g' pkg/sqlite_web_core.js
  
  # Add the rest of the template (everything after JS_GLUE_PLACEHOLDER)
  sed '1,/JS_GLUE_PLACEHOLDER/d' crates/sqlite-web/src/embedded_worker.js
} | awk 'BEGIN{getline b64<"pkg/sqlite_web_core_bg.wasm.b64"} {gsub(/__WASM_B64_CORE__/, b64)}1' > crates/sqlite-web/src/embedded_worker.js.final

# Replace the original with the final version
mv crates/sqlite-web/src/embedded_worker.js.final crates/sqlite-web/src/embedded_worker.js

echo "✅ Core embedding complete! Generated crates/sqlite-web/src/embedded_worker.js"
echo "📊 Embedded WASM size: $(wc -c < pkg/sqlite_web_core_bg.wasm.b64) base64 characters"
echo "📊 JS glue code lines: $(wc -l < pkg/sqlite_web_core.js)"

echo ""
echo "📦 Step 2: Building main package with embedded core..."
cd crates/sqlite-web
wasm-pack build --target web --out-dir ../../pkg
cd ../..

echo "🔧 Ensuring scoped package metadata..."
if grep -q '"name": "sqlite-web"' pkg/package.json; then
  sed -i.bak 's/"name": "sqlite-web"/"name": "@rainlanguage\/sqlite-web"/' pkg/package.json
  rm pkg/package.json.bak
  echo "Updated package name to @rainlanguage/sqlite-web"
else
  echo "Package name already scoped."
fi

echo "📌 Syncing package version with Cargo manifest..."
CARGO_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/sqlite-web/Cargo.toml | head -n 1)
if [ -n "$CARGO_VERSION" ]; then
  sed -i.bak "s/\"version\": \"[^\"]*\"/\"version\": \"$CARGO_VERSION\"/" pkg/package.json
  rm pkg/package.json.bak
  echo "Package version set to $CARGO_VERSION"
else
  echo "Warning: Could not determine Cargo version" >&2
fi

# Package the result
echo "📦 Packaging with npm pack..."
cd pkg
PACK_FILE=$(npm pack --silent)
cd ..

# Update Svelte integration with fresh package
echo "🔄 Updating Svelte integration..."
cd packages/ui
npm pkg delete dependencies.sqlite-web 2>/dev/null || true
npm pkg delete dependencies."@rainlanguage/sqlite-web" 2>/dev/null || true
npm pkg set dependencies."@rainlanguage/sqlite-web"="file:../../pkg/${PACK_FILE}"
rm -rf node_modules package-lock.json
npm install
cd ../..

echo ""
echo "🚀 Your SQLite worker is now fully self-contained with workspace architecture!"
echo "   ✅ Core logic is in sqlite-web-core"
echo "   ✅ Public API is in sqlite-web" 
echo "   ✅ No circular dependencies"
echo "   ✅ Clean build process"

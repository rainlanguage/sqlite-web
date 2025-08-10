// Simple static file server using Bun
const server = Bun.serve({
  port: 3000,
  fetch(req) {
    const url = new URL(req.url);
    let filePath = url.pathname;
    
    // Default to index.html for root path
    if (filePath === '/') {
      filePath = '/test.html';
    }
    
    // Remove leading slash and serve from current directory
    const localPath = `.${filePath}`;
    
    try {
      const file = Bun.file(localPath);
      
      // Set appropriate content type
      const headers = new Headers();
      if (filePath.endsWith('.js')) {
        headers.set('Content-Type', 'application/javascript');
      } else if (filePath.endsWith('.wasm')) {
        headers.set('Content-Type', 'application/wasm');
      } else if (filePath.endsWith('.html')) {
        headers.set('Content-Type', 'text/html');
      }
      
      // Enable CORS for all requests
      headers.set('Access-Control-Allow-Origin', '*');
      headers.set('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
      headers.set('Access-Control-Allow-Headers', 'Content-Type');
      
      return new Response(file, { headers });
    } catch (error) {
      console.error(`Error serving ${localPath}:`, error);
      return new Response(`File not found: ${filePath}`, { status: 404 });
    }
  },
});

console.log(`🚀 Server running at http://localhost:${server.port}`);
console.log(`📁 Serving files from: ${process.cwd()}`);
console.log(`🧪 Test page: http://localhost:${server.port}/test.html`);
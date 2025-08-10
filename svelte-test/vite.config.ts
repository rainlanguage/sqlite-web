import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import path from 'path';
import fs from 'fs';

export default defineConfig({
	plugins: [
		sveltekit(),
		{
			name: 'sqlite-worker-serve',
			configureServer(server) {
				server.middlewares.use('/pkg', (req, res, next) => {
					const filePath = req.url?.substring(1); // remove leading slash
					const fullPath = path.join(process.cwd(), 'node_modules/sqlite-worker', filePath || '');
					
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
	server: {
		fs: {
			allow: ['..']
		}
	},
	optimizeDeps: {
		exclude: ['sqlite-worker']
	},
	assetsInclude: ['**/*.wasm']
});

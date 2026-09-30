// Minimal static server for the SPA build (same behaviour as pixelplusd: files, else index.html).
// Usage: node scripts/serve.mjs [port]
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../build/', import.meta.url));
const port = Number(process.argv[2] ?? process.env.PORT ?? 4173);
const types = {
	'.html': 'text/html; charset=utf-8',
	'.js': 'text/javascript',
	'.css': 'text/css',
	'.svg': 'image/svg+xml',
	'.json': 'application/json',
	'.woff2': 'font/woff2',
	'.png': 'image/png',
	'.wasm': 'application/wasm'
};

createServer(async (req, res) => {
	const url = new URL(req.url ?? '/', 'http://x');
	if (url.pathname.startsWith('/api/')) {
		res.writeHead(503, { 'content-type': 'application/json' });
		return res.end('{"error":{"code":"no_daemon","message":"pixelplusd is not running"}}');
	}
	let file = normalize(join(root, decodeURIComponent(url.pathname)));
	if (!file.startsWith(root)) file = join(root, 'index.html');
	try {
		if ((await stat(file)).isDirectory()) file = join(file, 'index.html');
	} catch {
		file = join(root, 'index.html');
	}
	try {
		const body = await readFile(file);
		const immutable = file.includes('/_app/immutable/');
		res.writeHead(200, {
			'content-type': types[extname(file)] ?? 'application/octet-stream',
			'cache-control': immutable ? 'public, max-age=31536000, immutable' : 'no-cache'
		});
		res.end(body);
	} catch {
		res.writeHead(404);
		res.end('not found');
	}
}).listen(port, () => console.log(`serving build/ on http://localhost:${port}`));

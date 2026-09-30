import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

// During `pnpm dev` the API is proxied to a local pixelplusd (PIXELPLUS_API, default :8080).
// If it is not reachable the app falls back to the in-browser mock backend automatically.
const api = process.env.PIXELPLUS_API ?? 'http://127.0.0.1:8080';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		proxy: {
			'/api/v1/ws': { target: api.replace(/^http/, 'ws'), ws: true },
			'/api': { target: api, changeOrigin: true }
		}
	},
	build: {
		target: 'es2020',
		reportCompressedSize: false,
		chunkSizeWarningLimit: 1500,
		rollupOptions: {
			output: {
				// Fewer, larger requests matter more than perfect splitting when served from a Pi.
				manualChunks(id: string) {
					if (id.includes('@lucide/svelte')) return 'icons';
				}
			}
		}
	},
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});

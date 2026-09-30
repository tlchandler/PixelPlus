import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

// Tauri serves the built files from dist/; `pnpm dev` (port 1420) is used by `tauri dev`
// and also works in a plain browser with a mock backend (see src/lib/api.ts).
export default defineConfig({
	plugins: [svelte()],
	clearScreen: false,
	server: { port: 1420, strictPort: true },
	envPrefix: ['VITE_', 'TAURI_ENV_'],
	build: { target: 'es2021', outDir: 'dist', emptyOutDir: true, reportCompressedSize: false },
	test: { include: ['src/**/*.test.ts'], environment: 'node' }
});

import { defineConfig, devices } from '@playwright/test';

// UI suite against a REAL pixelplusd leader (not the demo backend). Start one first:
//   node ../scripts/e2e/run.mjs --keep       (3-node cluster with a realistic show)
//   pnpm test:real                           (PIXELPLUS_E2E_URL defaults to http://127.0.0.1:18080)
// Screenshots go to $SCREENS_DIR (default test-results/screens-real).
export default defineConfig({
	testDir: 'tests/e2e-real',
	timeout: 60_000,
	fullyParallel: false,
	workers: 1,
	retries: 0,
	reporter: [['list']],
	use: {
		baseURL: process.env.PIXELPLUS_E2E_URL ?? 'http://127.0.0.1:18080',
		trace: 'retain-on-failure',
		colorScheme: 'dark'
	},
	projects: [
		{ name: 'desktop', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
		{ name: 'phone', use: { ...devices['Pixel 7'], viewport: { width: 390, height: 844 } } }
	]
});

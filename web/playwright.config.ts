import { defineConfig, devices } from '@playwright/test';

// Smoke tests run against the production build served like pixelplusd does, in demo (mock) mode.
export default defineConfig({
	testDir: 'tests/e2e',
	timeout: 30_000,
	fullyParallel: true,
	retries: 0,
	reporter: [['list']],
	use: {
		baseURL: 'http://localhost:4174',
		trace: 'retain-on-failure'
	},
	projects: [
		{ name: 'desktop', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
		{ name: 'phone', use: { ...devices['Pixel 7'], viewport: { width: 390, height: 844 } } }
	],
	webServer: {
		command: 'test -f build/index.html || pnpm build; node scripts/serve.mjs 4174',
		url: 'http://localhost:4174',
		reuseExistingServer: !process.env.CI,
		timeout: 60_000
	}
});

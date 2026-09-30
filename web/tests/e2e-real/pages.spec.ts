// Walks every page of the UI against a REAL pixelplusd leader (see playwright.real.config.ts):
// no console errors, no failed API requests, real data on screen, a screenshot of each page.
import { expect, test, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';

const SCREENS = process.env.SCREENS_DIR ?? 'test-results/screens-real';
mkdirSync(SCREENS, { recursive: true });

/** [path, h1, a text only real data can produce] */
const pages: [string, string | RegExp, string | RegExp | null][] = [
	['/', /Good (morning|afternoon|evening)/, null],
	['/props', 'Props', 'Mega Tree'],
	['/layout', 'Layout', null],
	['/controllers', 'Controllers', 'Porch'],
	['/sequences', 'Sequences & Audio', 'E2E Song'],
	['/playlists', 'Playlists', 'Main Show'],
	['/schedule', 'Schedule', 'E2E tonight'],
	['/dj', 'DJ Studio', 'Welcome'],
	['/effects', 'Effects', 'Candy Cane'],
	['/games', 'Games', null],
	['/settings', 'Settings', null]
];

/** API answers that are expected on a development machine (no root helper, no Wi-Fi, no games sidecar). */
const EXPECTED_FAILURES: RegExp[] = [/\/system\/network\/scan/, /\/games\/(invite|stop|test)/];

export function watch(page: Page) {
	const problems: string[] = [];
	page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error' && !/Failed to load resource/.test(m.text())) problems.push(`console: ${m.text()}`);
	});
	page.on('response', (r) => {
		const url = r.url();
		if (r.status() >= 400 && !EXPECTED_FAILURES.some((re) => re.test(url)))
			problems.push(`${r.request().method()} ${url.replace(/^https?:\/\/[^/]+/, '')} → ${r.status()}`);
	});
	page.on('requestfailed', (r) => {
		const f = r.failure()?.errorText ?? '';
		// Navigations abort in-flight requests; that's not a failure.
		if (!/ERR_ABORTED|NS_BINDING_ABORTED/.test(f)) problems.push(`failed: ${r.url()} ${f}`);
	});
	return problems;
}

for (const [path, heading, marker] of pages) {
	test(`page ${path} works against the real daemon`, async ({ page }, info) => {
		const problems = watch(page);
		await page.goto(path);
		await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible();
		// The demo banner must not appear: this is the real backend.
		await expect(page.getByText(/demo mode/i)).toHaveCount(0);
		if (marker) await expect(page.getByText(marker).first()).toBeVisible({ timeout: 10_000 });
		await page.waitForTimeout(1500);
		const name = path === '/' ? 'dashboard' : path.slice(1);
		await page.screenshot({ path: `${SCREENS}/${name}-${info.project.name}.png`, fullPage: true });
		expect(problems).toEqual([]);
	});
}

test('public song request page', async ({ page }, info) => {
	const problems = watch(page);
	await page.goto('/request');
	await expect(page.getByText('E2E Song').first()).toBeVisible();
	await page.screenshot({ path: `${SCREENS}/request-${info.project.name}.png`, fullPage: true });
	expect(problems).toEqual([]);
});

test('follower UI shows who it follows', async ({ page }, info) => {
	const follower = (process.env.PIXELPLUS_E2E_URL ?? 'http://127.0.0.1:18080').replace(/:(\d+)$/, (_, p) => `:${+p + 1}`);
	const problems = watch(page);
	await page.goto(follower + '/');
	await expect(page.getByRole('heading', { name: /follows/ })).toContainText('Main');
	await page.screenshot({ path: `${SCREENS}/follower-${info.project.name}.png`, fullPage: true });
	expect(problems).toEqual([]);
});

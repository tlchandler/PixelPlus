// Buttons that call the API, clicked in the real UI, with the effect checked on the REAL
// daemon (its /player status and the output tap GET /debug/output). Desktop only.
import { expect, test, type Page } from '@playwright/test';
import { watch } from './helpers';

const BASE = process.env.PIXELPLUS_E2E_URL ?? 'http://127.0.0.1:18080';
const API = `${BASE}/api/v1`;
const HEADERS = { 'X-PixelPlus-Request': '1', 'content-type': 'application/json' };

async function daemon<T = any>(path: string, init?: RequestInit): Promise<T> {
	const r = await fetch(API + path, { ...init, headers: { ...HEADERS, ...(init?.headers ?? {}) } });
	if (!r.ok) throw new Error(`${path} → ${r.status} ${await r.text()}`);
	return r.json();
}
const player = () => daemon('/player');

test.describe.configure({ mode: 'serial' });
test.skip(({ isMobile }) => !!isMobile, 'desktop layout');

test.beforeEach(async () => {
	// Known state: show playing, no test, no blackout.
	await daemon('/test/stop', { method: 'POST', body: '{}' });
	await daemon('/player/blackout', { method: 'POST', body: '{"enabled":false}' });
	const seqs = await daemon<{ id: string }[]>('/sequences');
	await daemon('/player/play', { method: 'POST', body: JSON.stringify({ sequenceId: seqs[0].id }) });
});

async function expectNoProblems(page: Page, problems: string[]) {
	await page.waitForTimeout(300);
	expect(problems).toEqual([]);
}

test('transport: pause, resume, stop', async ({ page }) => {
	const problems = watch(page);
	await page.goto('/');
	const bar = page.getByRole('region', { name: 'Player' });
	await bar.getByRole('button', { name: 'Pause', exact: true }).first().click();
	await expect.poll(async () => (await player()).state).toBe('paused');
	await bar.getByRole('button', { name: 'Play', exact: true }).first().click();
	await expect.poll(async () => (await player()).state).toBe('playing');
	await bar.getByRole('button', { name: 'Stop', exact: true }).first().click();
	// Inside a scheduled show window stopping asks first.
	const confirmStop = page.getByRole('button', { name: 'Stop the show' });
	if (await confirmStop.isVisible({ timeout: 1500 }).catch(() => false)) await confirmStop.click();
	// Stop fades out (a few seconds) before the player is idle.
	await expect.poll(async () => (await player()).state, { timeout: 15_000 }).toBe('idle');
	await expectNoProblems(page, problems);
});

test('dashboard blackout and "Test all props"', async ({ page }) => {
	const problems = watch(page);
	await page.goto('/');
	// "Blackout" in the API; the UI may call it "Lights off".
	await page
		.getByRole('button', { name: /Blackout|Lights off/ })
		.first()
		.click();
	await expect.poll(async () => (await player()).blackout).toBe(true);
	await page
		.getByRole('button', { name: /Blackout|Lights (off|on)|back on/ })
		.first()
		.click();
	await expect.poll(async () => (await player()).blackout).toBe(false);
	await page.getByRole('button', { name: 'Test all props' }).click();
	await expect.poll(async () => (await player()).state).toBe('testing');
	await page.getByRole('button', { name: 'Stop test' }).first().click();
	await expect.poll(async () => (await player()).state).not.toBe('testing');
	await expectNoProblems(page, problems);
});

test('prop drawer test pattern lights the prop on its follower', async ({ page }) => {
	const problems = watch(page);
	await page.goto('/props');
	await page.getByRole('button', { name: 'Open Big Arch' }).click();
	await page.getByRole('radio', { name: 'Test' }).click();
	await page.getByRole('button', { name: 'Red', exact: true }).click();
	const follower = BASE.replace(/:(\d+)$/, (_, p) => `:${+p + 1}`);
	await expect
		.poll(async () => {
			const t = await (await fetch(`${follower}/api/v1/debug/output?outputs=1`)).json();
			const rgb = Buffer.from(t.outputs[0].rgb, 'base64');
			return rgb.subarray(0, 3).toString('hex');
		})
		.toBe('ff0000');
	await page.getByRole('button', { name: 'Stop test' }).first().click();
	await expect.poll(async () => (await player()).state).not.toBe('testing');
	await expectNoProblems(page, problems);
});

test('effects: apply a look live, then stop it', async ({ page }) => {
	const problems = watch(page);
	await page.goto('/effects');
	await page.getByRole('button', { name: 'Apply' }).first().click();
	await expect.poll(async () => (await player()).state).toBe('effect');
	await page.getByRole('button', { name: 'Live' }).first().click();
	await expect.poll(async () => (await player()).state).not.toBe('effect');
	await expectNoProblems(page, problems);
});

test('settings: take a snapshot and see it listed', async ({ page }) => {
	const problems = watch(page);
	await page.goto('/settings');
	await page
		.getByRole('button', { name: /Time machine|Backups/ })
		.first()
		.click();
	const before = (await daemon<unknown[]>('/snapshots')).length;
	await page
		.getByRole('button', { name: /Take snapshot|Back up now/ })
		.first()
		.click();
	const dialog = page.getByRole('dialog');
	if (await dialog.isVisible().catch(() => false)) {
		await dialog.getByRole('textbox').first().fill('From the UI test');
		await dialog
			.getByRole('button', { name: /Take|Save|Create/ })
			.last()
			.click();
	}
	await expect.poll(async () => (await daemon<unknown[]>('/snapshots')).length).toBe(before + 1);
	await expectNoProblems(page, problems);
});

test('health check re-runs from the dashboard', async ({ page }) => {
	const problems = watch(page);
	await page.goto('/');
	const before = (await daemon('/health')).ranAt;
	await page.getByRole('button', { name: 'Run health check' }).first().click();
	await expect.poll(async () => (await daemon('/health')).ranAt).not.toBe(before);
	await expectNoProblems(page, problems);
});

test('public song request from the phone page lands in the queue', async ({ page }) => {
	const problems = watch(page);
	await daemon('/show/settings', { method: 'PUT', body: JSON.stringify({ requests: { enabled: true } }) });
	for (const r of await daemon<{ id: string }[]>('/requests'))
		await fetch(`${API}/requests/${r.id}`, { method: 'DELETE', headers: HEADERS });
	await page.goto('/request');
	await page.getByPlaceholder(/first name/i).fill('Playwright');
	await page.getByRole('button', { name: 'Request' }).last().click();
	await expect
		.poll(async () => (await daemon<{ requestedBy?: string }[]>('/requests')).map((r) => r.requestedBy))
		.toContain('Playwright');
	await expectNoProblems(page, problems);
});

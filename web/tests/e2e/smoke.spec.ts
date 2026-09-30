import { expect, test, type Page } from '@playwright/test';

const pages: [string, string | RegExp][] = [
	['/', 'Chandler Lights'],
	['/props', 'Props'],
	['/layout', 'Layout'],
	['/controllers', 'Controllers'],
	['/sequences', 'Sequences & Audio'],
	['/playlists', 'Playlists'],
	['/schedule', 'Schedule'],
	['/dj', 'DJ Studio'],
	['/effects', 'Effects'],
	['/games', 'Games'],
	['/settings', 'Settings']
];

function watchErrors(page: Page) {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error') errors.push(`console: ${m.text()}`);
	});
	return errors;
}

for (const [path, heading] of pages) {
	test(`admin page ${path} renders without errors`, async ({ page }) => {
		const errors = watchErrors(page);
		await page.goto(`${path}?mock=1`);
		await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible();
		await page.waitForTimeout(800);
		expect(errors).toEqual([]);
	});
}

test('client-side navigation visits every page without errors', async ({ page, isMobile }) => {
	test.skip(!!isMobile, 'uses the desktop sidebar');
	const errors = watchErrors(page);
	await page.goto('/?mock=1');
	for (const [path, heading] of pages.slice(1)) {
		await page.locator(`nav[aria-label="Main"] a[href="${path}"]`).first().click();
		await expect(page).toHaveURL(new RegExp(`${path}$`));
		await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible();
	}
	expect(errors).toEqual([]);
});

test('prop drawer shows a readable wiring chain', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/props?mock=1');
	await page.getByRole('button', { name: 'Open Arch 5' }).click();
	await page.getByRole('radio', { name: 'Wiring' }).click();
	await expect(page.getByLabel('Wiring path').first()).toContainText('Main Controller');
	await expect(page.getByLabel('Wiring path').first()).toContainText('Front Yard receiver');
	await expect(page.getByLabel('Wiring path').first()).toContainText('pixels 51–100');
	expect(errors).toEqual([]);
});

test('transport bar toggles playback', async ({ page, isMobile }) => {
	test.skip(!!isMobile, 'desktop transport');
	await page.goto('/?mock=1');
	const bar = page.getByRole('region', { name: 'Player' });
	await bar.getByRole('button', { name: 'Pause', exact: true }).first().click();
	await expect(bar.getByRole('button', { name: 'Play', exact: true }).first()).toBeVisible();
	await bar.getByRole('button', { name: 'Play', exact: true }).first().click();
	await expect(bar.getByRole('button', { name: 'Pause', exact: true }).first()).toBeVisible();
});

test('setup wizard walks through to the checklist', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/setup?mock=1&setup=1');
	await page.getByRole('button', { name: /Get started/ }).click();
	await page.getByRole('button', { name: /Continue/ }).click();
	await expect(page.getByRole('heading', { name: '60-Port Transmitter' })).toBeVisible();
	await page.getByRole('button', { name: /Looks right/ }).click();
	await page.getByPlaceholder('e.g. Chandler Family Lights').fill('Test Lights');
	await page.getByRole('button', { name: /Continue/ }).click();
	await page.getByRole('button', { name: 'Skip' }).click();
	await expect(page.getByRole('heading', { name: 'Test Lights is ready' })).toBeVisible();
	expect(errors).toEqual([]);
});

test('public request page lets a visitor request a song', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/request?mock=1');
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Request a song');
	await page.getByLabel('Your first name').fill('Sam');
	await page.getByRole('button', { name: 'Request' }).nth(1).click();
	await expect(page.getByText('You’re on the list!')).toBeVisible();
	expect(errors).toEqual([]);
});

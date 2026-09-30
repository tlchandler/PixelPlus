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
	// Feature wave.
	['/reports', 'Reports'],
	['/map', 'Map my yard'],
	['/settings/seasons', 'Seasons'],
	['/calibrate', 'Sync lights to sound'],
	['/settings', 'Settings']
];

const SETTINGS_PAGES: [string, string][] = [
	['/settings/features', 'Features'],
	['/settings/https', 'Secure connection'],
	['/settings/remote', 'Remote access'],
	['/settings/power', 'Power'],
	['/settings/updates', 'Updates'],
	['/settings/xlights', 'xLights'],
	['/settings/seasons', 'Seasons'],
	['/settings/sensors', 'Sensors'],
	['/settings/reports', 'Nightly report'],
	['/settings/triggers', 'Triggers']
];

test('settings "More" links open their pages', async ({ page, isMobile }) => {
	test.skip(!!isMobile, 'desktop settings list');
	const errors = watchErrors(page);
	await page.goto('/settings?mock=1');
	for (const [path, heading] of SETTINGS_PAGES) {
		await page.locator(`nav[aria-label="Settings sections"] a[href="${path}"]`).click();
		await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible();
		await page.goBack();
	}
	expect(errors).toEqual([]);
});

test('the trust page opens without signing in', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/trust?mock=1');
	await expect(page.getByRole('heading', { level: 1, name: 'Make this phone trusted' })).toBeVisible();
	expect(errors).toEqual([]);
});

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
	await expect(page.getByRole('heading', { name: 'What will you use?' })).toBeVisible();
	await expect(page.getByRole('button', { name: /Continue/ })).toHaveCount(1);
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

test('controllers show how well each follower keeps time', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/controllers?mock=1');
	const badge = page.getByRole('button', { name: /In sync ±\d/ }).first();
	await expect(badge).toBeVisible();
	await badge.click();
	const details = page.getByRole('dialog', { name: 'Timing details' });
	await expect(details).toContainText('Clock accuracy');
	await expect(details).toContainText('Network round trip');
	await page.keyboard.press('Escape');
	await expect(details).toBeHidden();
	expect(errors).toEqual([]);
});

test('sync lights to sound: start the test, move the delay, done', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/settings?mock=1#audio');
	await page.getByRole('button', { name: /Sync lights to sound/ }).click();
	const wizard = page.getByRole('dialog', { name: 'Sync lights to sound' });
	await wizard.getByRole('button', { name: 'Start the test' }).click();
	await expect(wizard.getByText('Flashing and clicking')).toBeVisible();
	await wizard.getByRole('button', { name: 'FM transmitter' }).click();
	await wizard.getByRole('button', { name: '10 ms later' }).click();
	await expect(wizard.getByText('+30 ms', { exact: true }).first()).toBeVisible();
	await wizard.getByRole('button', { name: 'Done' }).click();
	await expect(wizard).toBeHidden();
	await expect(page.getByText('+30 ms', { exact: true })).toBeVisible();
	expect(errors).toEqual([]);
});

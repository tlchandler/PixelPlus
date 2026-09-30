// WS4 demo flows (mock backend): camera mapping with the simulated yard, the
// pixel-count check by tapping, and the guided "Add receiver" wizard.
import { expect, test, type Page } from '@playwright/test';

function watchErrors(page: Page) {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error') errors.push(`console: ${m.text()}`);
	});
	return errors;
}

test('map my yard: demo scan finds the props and applies a fix', async ({ page }) => {
	test.setTimeout(120_000);
	const errors = watchErrors(page);
	await page.goto('/map?mock=1');
	await expect(page.getByRole('heading', { level: 1, name: 'Map my yard' })).toBeVisible();
	await page.getByRole('button', { name: /Next: the camera/ }).click();
	await page.getByRole('button', { name: /Start the demo camera/ }).click();
	await page.getByRole('button', { name: /^Map my yard/ }).click();
	await expect(page.getByRole('heading', { name: /Found \d+ of \d+ props/ })).toBeVisible({
		timeout: 90_000
	});
	// The demo yard has one string wired backwards.
	await expect(page.getByText(/runs backwards/).first()).toBeVisible();
	await page.getByRole('button', { name: /Apply \d+ selected/ }).click();
	await expect(page.getByText(/Applied \d+ change/)).toBeVisible();
	expect(errors).toEqual([]);
});

test('pixel count by tapping finds the short string', async ({ page, isMobile }) => {
	test.skip(!!isMobile, 'list view actions are desktop');
	const errors = watchErrors(page);
	await page.goto('/props?mock=1');
	await page.getByRole('radio', { name: 'List' }).click();
	await page
		.getByRole('button', { name: /Check pixel count of/ })
		.first()
		.click();
	const dialog = page.getByRole('dialog');
	await dialog.getByRole('button', { name: /By looking/ }).click();
	// Keep seeing the red pixel: the search ends at the longest probe.
	await expect(dialog.locator('.question')).toBeVisible();
	const done = dialog.locator('.big', { hasText: 'pixels answer' });
	for (let i = 0; i < 16 && !(await done.isVisible()); i++) {
		await dialog
			.getByRole('button', { name: /I see it/ })
			.click({ timeout: 2000 })
			.catch(() => {});
		await page.waitForTimeout(100);
	}
	await expect(done).toBeVisible();
	await dialog.getByRole('button', { name: /Update to \d+/ }).click();
	await expect(page.getByText(/now has \d+ pixels/)).toBeVisible();
	expect(errors).toEqual([]);
});

test('guided add receiver: find the jack, pick a prop, create', async ({ page, isMobile }) => {
	test.skip(!!isMobile, 'desktop header action');
	const errors = watchErrors(page);
	await page.goto('/controllers?mock=1');
	await page.getByRole('button', { name: 'Add receiver' }).first().click();
	const dialog = page.getByRole('dialog');
	const next = dialog.getByRole('button', { name: /^Next/ });
	if (await next.isVisible()) await next.click();
	await dialog.getByRole('button', { name: /plugged in/ }).click();
	await dialog.getByRole('button', { name: /White, 1 blink/ }).click();
	// More than 8 free jacks: a second round.
	const again = dialog.getByRole('button', { name: /White, 1 blink/ });
	if (await again.isVisible().catch(() => false)) await again.click();
	await expect(dialog.getByText(/Found it: jack J\d+/)).toBeVisible();
	await dialog.getByRole('button', { name: /Next: its ports/ }).click();
	await dialog.locator('.props .opt').first().click();
	await dialog.getByRole('button', { name: 'At the cable end' }).click();
	await dialog.getByRole('button', { name: 'green' }).click();
	await dialog.getByRole('button', { name: 'red' }).click();
	for (let i = 0; i < 3; i++) await dialog.getByRole('button', { name: 'Nothing lit' }).click();
	await dialog.getByRole('button', { name: /Create receiver/ }).click();
	await expect(page.getByText(/added on J\d+/)).toBeVisible();
	expect(errors).toEqual([]);
});

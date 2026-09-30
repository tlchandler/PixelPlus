// WS2 UI in demo mode: tags and bulk tagging, sequence preview, auto light shows,
// smart playlists and the Layout page preview (F2, F3, F18).
import { expect, test, type Page } from '@playwright/test';

function watchErrors(page: Page) {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error') errors.push(`console: ${m.text()}`);
	});
	return errors;
}

test('tag filter and bulk tagging on the Sequences page', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/sequences?mock=1');
	const bar = page.getByRole('group', { name: 'Filter by tag' });
	await expect(bar.getByRole('button', { name: /^kids/ })).toBeVisible();
	// Filter: only kids songs stay.
	await bar.getByRole('button', { name: /^kids/ }).click();
	await expect(page.locator('.srow', { hasText: 'Wizards in Winter' })).toHaveCount(0);
	await expect(page.locator('.srow', { hasText: 'Let It Go' }).first()).toBeVisible();
	await bar.getByRole('button', { name: 'Clear' }).click();
	// Bulk: select two songs and add a tag.
	await page.getByRole('button', { name: 'Select songs to tag' }).click();
	await page.getByRole('checkbox', { name: 'Select Wizards in Winter' }).check();
	await page.getByRole('checkbox', { name: 'Select Carol of the Bells' }).check();
	await page.getByRole('combobox', { name: 'Tag to add or remove' }).fill('Neighbors');
	await page.getByRole('button', { name: 'Add tag' }).click();
	await expect(page.getByText('Tagged 2 items')).toBeVisible();
	await expect(bar.getByRole('button', { name: /^neighbors/ })).toBeVisible();
	await page.getByRole('button', { name: 'Stop selecting' }).click();
	expect(errors).toEqual([]);
});

test('preview a sequence on the phone without touching the lights', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/sequences?mock=1');
	await page.getByRole('button', { name: 'Preview Wizards in Winter on this device' }).click();
	const dialog = page.getByRole('dialog');
	await expect(dialog.getByText('your lights are not affected')).toBeVisible();
	const play = dialog.getByRole('button', { name: /Pause preview|Play preview/ });
	await expect(play).toBeEnabled();
	// Autoplay: the position advances.
	const slider = dialog.getByRole('slider', { name: 'Preview position' });
	await expect
		.poll(async () => Number(await slider.getAttribute('aria-valuenow')), { timeout: 8000 })
		.toBeGreaterThan(0);
	await dialog.getByRole('button', { name: 'Pause preview' }).click();
	await expect(dialog.getByRole('button', { name: 'Play preview' })).toBeVisible();
	expect(errors).toEqual([]);
});

test('make a light show for a song', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/sequences?mock=1');
	await page.getByRole('radio', { name: /Audio/ }).click();
	await page.getByRole('button', { name: /Make a light show for Jingle Bell Rock/ }).click();
	const dialog = page.getByRole('dialog', { name: 'Make a light show' });
	await expect(dialog.getByText('BPM')).toBeVisible();
	await dialog.getByText('Candy', { exact: true }).click();
	await dialog.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByText('Your light show is ready')).toBeVisible({ timeout: 10000 });
	await expect(page.locator('.srow', { hasText: 'Jingle Bell Rock (Candy light show)' })).toBeVisible();
	expect(errors).toEqual([]);
});

test('a smart playlist shows tonight’s line-up', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/playlists?mock=1');
	await page.getByRole('switch', { name: 'Smart' }).click();
	const rules = page.getByTestId('smart-rules');
	await expect(rules).toBeVisible();
	await rules.getByRole('button', { name: 'kids', exact: true }).first().click();
	const tonight = page.getByTestId('smart-tonight');
	await expect(tonight.getByText('Let It Go')).toBeVisible();
	await expect(tonight.getByText('Wizards in Winter')).toHaveCount(0);
	expect(errors).toEqual([]);
});

test('the Layout page previews a sequence', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/layout?mock=1');
	await page.getByRole('radio', { name: 'Preview' }).click();
	await expect(page.getByLabel('Sequence to preview')).toBeVisible();
	await expect(page.getByTestId('preview-transport')).toBeVisible();
	await expect(page.getByRole('button', { name: /Play preview|Pause preview/ })).toBeEnabled({
		timeout: 8000
	});
	await expect(page).toHaveURL(/preview=/);
	await page.getByRole('button', { name: 'Back to live' }).click();
	await expect(page.getByTestId('preview-transport')).toHaveCount(0);
	expect(errors).toEqual([]);
});

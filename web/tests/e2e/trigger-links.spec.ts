// Settings → Triggers → "Connect Home Assistant & other devices" (secret trigger links,
// ARCHITECTURE §12.18): make / rotate / revoke a link, the token shown once with copy
// buttons, Home Assistant and curl snippets, QR code, the two "widen it" switches, and a
// phone-sized layout. Demo backend (mock mode); state lives for the life of the tab.
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';

function watchErrors(page: Page) {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error') errors.push(`console: ${m.text()}`);
	});
	return errors;
}

async function openTriggers(page: Page, theme?: 'light' | 'dark') {
	if (theme)
		await page.addInitScript((t) => {
			try {
				localStorage.setItem('pp-theme', t);
			} catch {
				/* ignore */
			}
		}, theme);
	await page.goto('/settings/triggers?mock=1');
	await expect(page.getByRole('heading', { level: 1, name: 'Triggers' })).toBeVisible();
	const panel = page.getByTestId('trigger-link-panel').first();
	await expect(panel).toBeVisible();
	return panel;
}

test('make, copy, rotate and revoke a trigger link', async ({ page, context, browserName }) => {
	const errors = watchErrors(page);
	if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
	const panel = await openTriggers(page);
	await expect(panel).toContainText('Secret link on');
	await panel.getByRole('button', { name: /Connect Home Assistant/ }).click();
	// The demo link exists already: the token is hidden, the snippet has a placeholder.
	await expect(panel.getByTestId('link-token')).toHaveCount(0);
	await expect(panel.getByTestId('link-snippet')).toContainText('PASTE-YOUR-TOKEN-HERE');
	await expect(panel.getByTestId('link-url')).toHaveText(
		'http://pixelplus.local/api/v1/hooks/trigger/trhass0001'
	);
	await expect(panel.getByTestId('link-last-use')).toContainText('192.168.1.20');

	// Rotate: confirm, then the new token is shown once.
	await panel.getByRole('button', { name: 'Make a new link' }).click();
	await page.getByRole('dialog').getByRole('button', { name: 'Make a new link' }).click();
	const token = panel.getByTestId('link-token');
	await expect(token).toHaveText(/^ppt_[\w-]{43}$/);
	const tok = (await token.textContent())!;
	await expect(panel).toContainText('Copy it now');
	const snippet = panel.getByTestId('link-snippet');
	await expect(snippet).toContainText('rest_command:');
	await expect(snippet).toContainText(`Authorization: "Bearer ${tok}"`);
	await expect(panel).toContainText(`ends in …${tok.slice(-4)}`);
	await panel.getByRole('button', { name: 'Copy token' }).click();
	await expect(page.getByText('Token copied')).toBeVisible();
	if (browserName === 'chromium') expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(tok);

	await panel.getByRole('radio', { name: 'curl' }).click();
	await expect(snippet).toContainText(`curl -X POST -H 'Authorization: Bearer ${tok}'`);
	await panel.getByRole('radio', { name: 'One link' }).click();
	await expect(snippet).toContainText(`?token=${tok}`);
	await panel.getByRole('button', { name: /Show QR code/ }).click();
	await expect(panel.getByRole('img', { name: /QR code for/ })).toBeVisible();

	// The switches explain their risk.
	await panel.getByRole('switch', { name: 'Allow simple GET links' }).click();
	await expect(panel).toContainText('open links on their own');
	await panel.getByRole('switch', { name: 'Allow from the internet' }).click();
	await expect(panel).toContainText('Anyone on the internet');

	// Hide the token: it's gone for good.
	await panel.getByRole('button', { name: 'Done — hide the token' }).click();
	await expect(panel.getByTestId('link-token')).toHaveCount(0);

	// Revoke.
	await panel.getByRole('button', { name: 'Turn off link' }).click();
	await page.getByRole('dialog').getByRole('button', { name: 'Turn off link' }).click();
	await expect(panel.getByRole('button', { name: 'Make a secret link' })).toBeVisible();
	await expect(panel).not.toContainText('Secret link on');
	expect(errors).toEqual([]);
});

test('a new web-link trigger gets its first link', async ({ page }) => {
	await openTriggers(page);
	await page.getByRole('button', { name: 'Web link' }).click();
	const panel = page.getByTestId('trigger-link-panel').last();
	await panel.getByRole('button', { name: /Connect Home Assistant/ }).click();
	const make = panel.getByRole('button', { name: 'Make a secret link' });
	await expect(make).toBeEnabled({ timeout: 5000 }); // once the trigger is saved
	await make.click();
	await expect(panel.getByTestId('link-token')).toHaveText(/^ppt_/);
	await expect(panel.getByTestId('link-snippet')).toContainText('pixelplus_new_trigger');
});

test.describe('phone', () => {
	test.beforeEach(({ isMobile }) => test.skip(!isMobile, 'phone only'));
	test('the panel fits a 390 px screen', async ({ page }) => {
		const panel = await openTriggers(page);
		await panel.getByRole('button', { name: /Connect Home Assistant/ }).click();
		await panel.getByRole('button', { name: 'Make a new link' }).click();
		await page.getByRole('dialog').getByRole('button', { name: 'Make a new link' }).click();
		await expect(panel.getByTestId('link-token')).toBeVisible();
		const o = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
		expect(o).toBeLessThanOrEqual(0);
		const box = await panel.getByRole('button', { name: 'Copy token' }).boundingBox();
		expect(box!.x + box!.width).toBeLessThanOrEqual(390);
	});
});

test.describe('contrast (axe, WCAG AA)', () => {
	test.beforeEach(({ isMobile }) => test.skip(!!isMobile, 'desktop covers the same tokens'));
	for (const theme of ['dark', 'light'] as const)
		test(`trigger link panel in ${theme}`, async ({ page }) => {
			const panel = await openTriggers(page, theme);
			await panel.getByRole('button', { name: /Connect Home Assistant/ }).click();
			await panel.getByRole('button', { name: 'Make a new link' }).click();
			await page.getByRole('dialog').getByRole('button', { name: 'Make a new link' }).click();
			await panel.getByRole('switch', { name: 'Allow simple GET links' }).click();
			await page.waitForTimeout(6000); // let the toasts go
			const res = await new AxeBuilder({ page })
				.include('[data-testid="trigger-link-panel"]')
				.withRules(['color-contrast'])
				.exclude('svg')
				.analyze();
			const bad = res.violations.flatMap((v) =>
				v.nodes.map((n) => `${n.target.join(' ')}: ${n.any[0]?.message ?? v.help}`)
			);
			expect(bad).toEqual([]);
		});
});

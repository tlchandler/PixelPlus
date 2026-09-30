// Settings → Features: turning features off trims the interface (navigation, pages, dashboard,
// buttons in other pages); a turned-off page explains itself and turns back on in one tap.
// The demo backend keeps its state for the life of the tab, so these tests move around with
// in-app navigation (a reload starts the demo show afresh).
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

async function openFeatures(page: Page, theme?: 'light' | 'dark') {
	if (theme)
		await page.addInitScript((t) => {
			try {
				localStorage.setItem('pp-theme', t);
			} catch {
				/* ignore */
			}
		}, theme);
	await page.goto('/settings/features?mock=1');
	await expect(page.getByRole('heading', { level: 1, name: 'Features' })).toBeVisible();
}

/** Flip a feature's switch, confirming when PixelPlus asks. */
async function flip(page: Page, name: string, on: boolean) {
	const sw = page.getByRole('switch', { name, exact: true });
	await expect(sw).toHaveAttribute('aria-checked', String(!on));
	await sw.click();
	const dlg = page.getByRole('dialog');
	if (!on && (await dlg.isVisible().catch(() => false)))
		await dlg.getByRole('button', { name: 'Turn off' }).click();
	await expect(sw).toHaveAttribute('aria-checked', String(on));
}

/** Navigate inside the app (keeps the demo state). */
async function go(page: Page, isMobile: boolean, href: string) {
	if (isMobile) {
		const tab = page.locator(`nav.tabbar a[href="${href}"]`);
		if (await tab.count()) await tab.click();
		else {
			await page.getByRole('button', { name: 'More pages' }).click();
			await page.getByRole('dialog', { name: 'More pages' }).locator(`a[href="${href}"]`).click();
		}
	} else await page.locator(`nav.sidebar a[href="${href}"]`).first().click();
	await expect(page).toHaveURL(new RegExp(`${href}$`));
}

test('turning a feature off removes it everywhere; turning it on brings it back', async ({
	page,
	isMobile
}) => {
	const errors = watchErrors(page);
	await openFeatures(page);
	await flip(page, 'Games', false);
	await flip(page, 'DJ Studio', false);
	await expect(page.getByText('DJ Studio is off')).toBeVisible();

	// Navigation (sidebar, or tab bar + More sheet on a phone) no longer lists them.
	if (isMobile) {
		await page.getByRole('button', { name: 'More pages' }).click();
		const sheet = page.getByRole('dialog', { name: 'More pages' });
		await expect(sheet.locator('a[href="/games"]')).toHaveCount(0);
		await expect(sheet.locator('a[href="/dj"]')).toHaveCount(0);
		await expect(sheet.locator('a[href="/effects"]')).toHaveCount(1);
		await expect(sheet.getByRole('link', { name: 'Customize what you see' })).toBeVisible();
		await page.keyboard.press('Escape');
	} else {
		const nav = page.locator('nav.sidebar');
		await expect(nav.locator('a[href="/games"]')).toHaveCount(0);
		await expect(nav.locator('a[href="/dj"]')).toHaveCount(0);
		await expect(nav.locator('a[href="/effects"]')).toHaveCount(1);
	}

	// The playlist editor offers no DJ clips or game items.
	await go(page, isMobile, '/playlists');
	await expect(page.getByRole('heading', { level: 1, name: 'Playlists' })).toBeVisible();
	await expect(page.getByRole('radio', { name: 'DJ', exact: true })).toHaveCount(0);

	// Back on: everything returns.
	await go(page, isMobile, '/settings');
	await page.locator('nav[aria-label="Settings sections"] a[href="/settings/features"]').click();
	await flip(page, 'Games', true);
	if (isMobile) {
		await page.getByRole('button', { name: 'More pages' }).click();
		await expect(page.getByRole('dialog', { name: 'More pages' }).locator('a[href="/games"]')).toHaveCount(1);
		await page.keyboard.press('Escape');
	} else await expect(page.locator('nav.sidebar a[href="/games"]')).toHaveCount(1);
	expect(errors).toEqual([]);
});

test('a turned-off page explains itself and turns back on', async ({ page, isMobile }) => {
	test.skip(!!isMobile, 'uses the desktop sidebar link');
	const errors = watchErrors(page);
	await page.goto('/games?mock=1');
	await expect(page.getByRole('heading', { level: 1, name: 'Games' })).toBeVisible();
	await page.getByRole('link', { name: 'Customize what you see' }).click();
	await flip(page, 'Games', false);
	await page.goBack();
	await expect(page).toHaveURL(/\/games(\?|$)/);
	await expect(page.getByRole('heading', { level: 1, name: 'Games is turned off' })).toBeVisible();
	await expect(page.getByText('Anything you set up is still here')).toBeVisible();
	await page.getByRole('button', { name: 'Turn on Games' }).click();
	await expect(page.getByRole('heading', { level: 1, name: 'Games' })).toBeVisible();
	await expect(page.locator('nav.sidebar a[href="/games"]')).toHaveCount(1);
	expect(errors).toEqual([]);
});

test('dependencies follow: phone trust off turns off Map my yard and Sync to sound', async ({ page }) => {
	await openFeatures(page);
	await page.getByRole('switch', { name: 'Phone trust (HTTPS)' }).click();
	const dlg = page.getByRole('dialog');
	await expect(dlg).toContainText('Map my yard and Sync to sound need it and will turn off too');
	await dlg.getByRole('button', { name: 'Turn off' }).click();
	await expect(page.getByRole('switch', { name: 'Map my yard' })).toHaveAttribute('aria-checked', 'false');
	await expect(page.getByRole('switch', { name: 'Sync to sound' })).toHaveAttribute('aria-checked', 'false');
	// Turning one back on brings what it needs.
	await page.getByRole('switch', { name: 'Map my yard' }).click();
	await expect(page.getByRole('switch', { name: 'Phone trust (HTTPS)' })).toHaveAttribute(
		'aria-checked',
		'true'
	);
	await expect(page.getByText(/came on too/)).toBeVisible();
});

test('presets: Essentials trims the dashboard and pages, Everything restores', async ({ page, isMobile }) => {
	const errors = watchErrors(page);
	await openFeatures(page);
	await expect(page.getByRole('radio', { name: /Everything/ })).toHaveAttribute('aria-checked', 'true');
	await page.getByRole('radio', { name: /Essentials/ }).click();
	const dlg = page.getByRole('dialog');
	await expect(dlg).toContainText('set up on this controller and will be turned off');
	await dlg.getByRole('button', { name: 'Switch' }).click();
	await expect(page.getByRole('radio', { name: /Essentials/ })).toHaveAttribute('aria-checked', 'true');
	await expect(page.getByRole('switch', { name: 'Fault finder' })).toHaveAttribute('aria-checked', 'true');
	await expect(page.getByRole('switch', { name: 'Seasons' })).toHaveAttribute('aria-checked', 'false');

	// Dashboard: no song-request card, no season chip.
	await go(page, isMobile, '/');
	await expect(page.getByRole('heading', { level: 1, name: 'Chandler Lights' })).toBeVisible();
	await expect(page.getByText('Song requests', { exact: true })).toHaveCount(0);
	await expect(page.locator('a.season-chip')).toHaveCount(0);

	// Props: no "Map my yard" button; Controllers: the receiver wizard is still there.
	await go(page, isMobile, '/props');
	await expect(page.getByRole('heading', { level: 1, name: 'Props' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Map my yard' })).toHaveCount(0);

	// Settings: song requests and the other off sections are gone; Features is first.
	await go(page, isMobile, '/settings');
	const list = page.locator('nav[aria-label="Settings sections"]');
	await expect(list.locator('a, button').first()).toContainText('Features');
	await expect(list.getByRole('button', { name: /Song requests/ })).toHaveCount(0);
	await expect(list.locator('a[href="/settings/seasons"]')).toHaveCount(0);
	await expect(list.locator('a[href="/settings/power"]')).toHaveCount(1);
	await expect(list.locator('a[href="/settings/updates"]')).toHaveCount(1);

	// Everything again.
	await list.locator('a[href="/settings/features"]').click();
	await page.getByRole('radio', { name: /Everything/ }).click();
	await expect(page.getByRole('radio', { name: /Everything/ })).toHaveAttribute('aria-checked', 'true');
	await go(page, isMobile, '/');
	await expect(page.getByText('Song requests', { exact: true })).toBeVisible();
	expect(errors).toEqual([]);
});

test('search finds features by what they do', async ({ page }) => {
	await openFeatures(page);
	await page.getByRole('searchbox', { name: 'Search features' }).fill('camera');
	await expect(page.getByRole('switch')).toHaveCount(2); // Map my yard, Phone trust
	await page.getByRole('searchbox', { name: 'Search features' }).fill('zzzz');
	await expect(page.getByText('No feature matches')).toBeVisible();
	await page.getByRole('button', { name: 'Show all features' }).click();
	await expect(page.getByRole('switch')).toHaveCount(24);
});

test('in-use facts are shown', async ({ page }) => {
	await openFeatures(page);
	const dj = page.locator('li', { has: page.getByRole('switch', { name: 'DJ Studio' }) });
	await expect(dj).toContainText('In use');
	await expect(dj).toContainText(/\d+ clips · \d+ voices · used in \d+ playlists?/);
	const games = page.locator('li', { has: page.getByRole('switch', { name: 'Games' }) });
	await expect(games).toContainText('on for visitors');
});

test('the setup wizard asks what you will use', async ({ page }) => {
	const errors = watchErrors(page);
	await page.goto('/setup?mock=1&setup=1');
	await page.getByRole('button', { name: /Get started/ }).click();
	await page.getByRole('button', { name: /Continue/ }).click();
	await page.getByRole('button', { name: /Looks right/ }).click();
	await page.getByPlaceholder('e.g. Chandler Family Lights').fill('Fresh Lights');
	await page.getByRole('button', { name: /Continue/ }).click();
	await expect(page.getByRole('heading', { name: 'What will you use?' })).toBeVisible();
	await expect(page.getByRole('button', { name: /Continue/ })).toHaveCount(1);
	await expect(page.getByRole('radio', { name: /Essentials/ })).toHaveAttribute('aria-checked', 'true');
	await expect(page.getByText('You can change this any time in')).toBeVisible();
	await page.getByRole('radio', { name: /Let me choose/ }).click();
	await page.getByRole('button', { name: 'Games', exact: true }).click();
	await expect(page.getByRole('button', { name: 'Games', exact: true })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await page.getByRole('button', { name: /Continue/ }).click();
	await page.getByRole('button', { name: 'Skip' }).click();
	await expect(page.getByRole('heading', { name: 'Fresh Lights is ready' })).toBeVisible();
	expect(errors).toEqual([]);
});

test.describe('phone', () => {
	test.beforeEach(({ isMobile }) => test.skip(!isMobile, 'phone only'));
	test('no sideways scrolling at 390 px and 44 px switches', async ({ page }) => {
		await openFeatures(page);
		const o = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
		expect(o).toBeLessThanOrEqual(0);
		const sizes = await page.getByRole('switch').evaluateAll((els) =>
			els.map((el) => {
				const r = el.getBoundingClientRect();
				const b = getComputedStyle(el, '::before');
				// The switch extends its hit area with an absolutely placed ::before.
				return Math.min(r.width + 8, r.height + 20) - (b.position === 'absolute' ? 0 : 99);
			})
		);
		expect(Math.min(...sizes)).toBeGreaterThanOrEqual(44);
		await page.getByRole('radio', { name: /Essentials/ }).click();
		await page.getByRole('dialog').getByRole('button', { name: 'Switch' }).click();
		const o2 = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
		expect(o2).toBeLessThanOrEqual(0);
	});
});

test.describe('contrast (axe, WCAG AA)', () => {
	test.beforeEach(({ isMobile }) => test.skip(!!isMobile, 'desktop covers the same tokens'));
	for (const theme of ['dark', 'light'] as const)
		test(`features page in ${theme}, with some features off`, async ({ page }) => {
			await openFeatures(page, theme);
			await flip(page, 'Games', false);
			await flip(page, 'Home Assistant & MQTT', false);
			await page.waitForTimeout(8000); // let the toasts go
			const res = await new AxeBuilder({ page }).withRules(['color-contrast']).exclude('svg').analyze();
			const bad = res.violations.flatMap((v) =>
				v.nodes.map((n) => `${n.target.join(' ')}: ${n.any[0]?.message ?? v.help}`)
			);
			expect(bad).toEqual([]);
		});
});

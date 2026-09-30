// UX guard rails: nothing scrolls sideways on a phone, text passes WCAG AA contrast in both
// themes, touch targets are big enough, and the first-run journey (empty show) works.
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';

const ADMIN = [
	'/',
	'/props',
	'/layout',
	'/controllers',
	'/sequences',
	'/playlists',
	'/schedule',
	'/dj',
	'/effects',
	'/games',
	'/settings',
	'/yard-sign'
];

async function open(page: Page, path: string, mock = '1', theme?: 'light' | 'dark') {
	if (theme)
		await page.addInitScript((t) => {
			try {
				localStorage.setItem('pp-theme', t);
			} catch {
				/* ignore */
			}
		}, theme);
	const [p, hash] = path.split('#');
	await page.goto(`${p}${p.includes('?') ? '&' : '?'}mock=${mock}${hash ? `#${hash}` : ''}`);
	await expect(page.getByRole('heading', { level: 1 }).first()).toBeAttached();
	await page.waitForTimeout(700);
}

async function horizontalOverflow(page: Page) {
	return page.evaluate(() => ({
		scroll: document.documentElement.scrollWidth,
		width: window.innerWidth,
		// The widest offenders, to make a failure easy to fix.
		culprits: [...document.querySelectorAll<HTMLElement>('body *')]
			.filter((el) => el.getBoundingClientRect().right > window.innerWidth + 1)
			.filter((el) => getComputedStyle(el).position !== 'fixed')
			.slice(0, 5)
			.map((el) => `${el.tagName.toLowerCase()}.${[...el.classList].join('.')}`)
	}));
}

test.describe('phone layout', () => {
	test.beforeEach(({ isMobile }) => test.skip(!isMobile, 'phone only'));

	const pages = [
		...ADMIN.map((p) => [p, '1'] as const),
		...ADMIN.map((p) => [p, 'empty'] as const),
		['/settings#network', '1'] as const,
		['/settings#requests', '1'] as const,
		['/settings#logs', '1'] as const,
		['/request', '1'] as const,
		['/setup?setup=1', '1'] as const
	];
	for (const [path, mock] of pages) {
		test(`no sideways scrolling at 390 px: ${path} (${mock === 'empty' ? 'empty show' : 'demo'})`, async ({
			page
		}) => {
			await open(page, path, mock);
			const o = await horizontalOverflow(page);
			expect(o.scroll, `wider than the screen: ${o.culprits.join(', ')}`).toBeLessThanOrEqual(o.width);
		});
	}

	test('tap targets are at least 44 px on touch screens', async ({ page }) => {
		const small: string[] = [];
		for (const path of ['/', '/props', '/playlists', '/schedule', '/settings#audio', '/games', '/dj']) {
			await open(page, path);
			const found = await page.evaluate(() =>
				[
					...document.querySelectorAll<HTMLElement>(
						'main button, main a.btn, main [role="radio"], main select, main input:not([type="checkbox"]):not([type="range"]), nav[aria-label="Main"] a'
					)
				]
					.filter((el) => {
						const r = el.getBoundingClientRect();
						const style = getComputedStyle(el);
						return r.width > 0 && r.height > 0 && style.visibility !== 'hidden' && !el.closest('[aria-hidden="true"]');
					})
					.filter((el) => {
						const r = el.getBoundingClientRect();
						// Switches extend their hit area with a pseudo-element; count that.
						const before = getComputedStyle(el, '::before');
						const extra = before.content !== 'none' && before.position === 'absolute' ? 12 : 0;
						return Math.min(r.height, r.width) + extra < 43.5;
					})
					.map((el) => `${el.tagName.toLowerCase()}[${el.getAttribute('aria-label') ?? el.textContent?.trim().slice(0, 24)}]`)
			);
			small.push(...found.map((f) => `${path} ${f}`));
		}
		expect(small).toEqual([]);
	});

	test('settings open as a list of sections, then drill in', async ({ page }) => {
		await open(page, '/settings');
		await page.getByRole('button', { name: /Backups/ }).click();
		await expect(page.getByRole('heading', { level: 2, name: 'Backups' })).toBeVisible();
		await page.getByRole('button', { name: 'Settings' }).first().click();
		await expect(page.getByRole('button', { name: /Song requests/ })).toBeVisible();
	});

	test('schedule opens on the day list', async ({ page }) => {
		await open(page, '/schedule');
		await expect(page.getByRole('radio', { name: 'Days' })).toHaveAttribute('aria-checked', 'true');
	});

	test('the fault finder answers are full width and stacked', async ({ page }) => {
		await open(page, '/props');
		await page.getByRole('button', { name: 'Open Arch 5' }).click();
		await page.getByRole('radio', { name: 'Test' }).click();
		await page.getByRole('button', { name: 'Start', exact: true }).click();
		await page.getByRole('button', { name: /Start — light/ }).click();
		const yes = await page.getByRole('button', { name: 'Yes, all good' }).boundingBox();
		const no = await page.getByRole('button', { name: /No, something/ }).boundingBox();
		expect(yes!.width).toBeGreaterThan(300);
		expect(no!.y).toBeGreaterThan(yes!.y + yes!.height - 1);
		await expect(page.getByRole('button', { name: 'Stop looking' })).toBeInViewport();
	});
});

test.describe('contrast (axe, WCAG AA)', () => {
	test.beforeEach(({ isMobile }) => test.skip(!!isMobile, 'desktop covers the same tokens'));
	for (const theme of ['dark', 'light'] as const)
		for (const path of ['/', '/props', '/controllers', '/schedule', '/settings#requests', '/playlists', '/dj'])
			test(`${path} in ${theme}`, async ({ page }) => {
				await open(page, path, '1', theme);
				const res = await new AxeBuilder({ page })
					.withRules(['color-contrast'])
					// The live layout canvas and board art are pictures, not text on the page.
					.exclude('canvas')
					.exclude('svg')
					.analyze();
				const bad = res.violations.flatMap((v) =>
					v.nodes.map((n) => `${n.target.join(' ')}: ${n.any[0]?.message ?? v.help}`)
				);
				expect(bad).toEqual([]);
			});
});

test.describe('first run', () => {
	test('an empty show gets a "Get your show ready" checklist', async ({ page }) => {
		await open(page, '/', 'empty');
		const card = page.getByRole('region', { name: 'Get your show ready' });
		await expect(card).toBeVisible();
		await expect(card).toContainText('1 of 5 done');
		await card.getByRole('link', { name: /Import your xLights layout/ }).click();
		await expect(page.getByRole('dialog', { name: 'Import from xLights' })).toBeVisible();
	});

	test('the checklist can be hidden', async ({ page }) => {
		await open(page, '/', 'empty');
		await page.getByRole('button', { name: 'Hide this checklist' }).click();
		await expect(page.getByRole('region', { name: 'Get your show ready' })).toHaveCount(0);
	});

	test('the finished demo show has no checklist', async ({ page }) => {
		await open(page, '/');
		await expect(page.getByRole('region', { name: 'Get your show ready' })).toHaveCount(0);
	});
});

test.describe('lights off', () => {
	test('shows a banner and the dashboard badge says LIGHTS OFF', async ({ page, isMobile }) => {
		await open(page, '/');
		await page.getByRole('button', { name: 'Lights off', exact: true }).first().click();
		await expect(page.getByRole('status').filter({ hasText: 'Lights are off' }).first()).toBeVisible();
		await expect(page.getByText('LIGHTS OFF').first()).toBeVisible();
		await page.getByRole('button', { name: 'Turn lights back on' }).first().click();
		await expect(page.getByText('LIGHTS OFF')).toHaveCount(0);
		void isMobile;
	});
});

test('prop edits save by themselves, with one Undo', async ({ page }) => {
	await open(page, '/props');
	await page.getByRole('button', { name: 'Open Arch 5' }).click();
	const name = page.getByLabel('Name', { exact: true });
	await name.fill('Arch Five');
	await expect(page.getByRole('dialog').getByText('Saved')).toBeVisible({ timeout: 4000 });
	await page.getByRole('button', { name: 'Close panel' }).click();
	await expect(page.getByText('Saved changes to Arch Five')).toBeVisible();
	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(page.getByRole('button', { name: 'Open Arch 5' })).toBeVisible();
});

test('board diagrams never say FPP', async ({ page }) => {
	await open(page, '/controllers');
	await expect(page.locator('svg.board')).not.toHaveCount(0);
	const text = await page.locator('svg.board').allTextContents();
	expect(text.join(' ')).not.toMatch(/FPP|difftx/i);
	expect(text.join(' ')).toContain('PixelPlus');
});

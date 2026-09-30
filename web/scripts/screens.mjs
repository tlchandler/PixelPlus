// Capture screenshots of every page in demo (mock) mode.
// Usage: node scripts/screens.mjs [baseUrl] [outDir] [filter]
import { chromium } from '@playwright/test';
import { mkdirSync } from 'node:fs';

const base = process.argv[2] ?? 'http://localhost:4173';
const out = process.argv[3] ?? 'screens';
const filter = process.argv[4];
mkdirSync(out, { recursive: true });

const pages = [
	['dashboard', '/'],
	['props', '/props'],
	['layout', '/layout'],
	['controllers', '/controllers'],
	['sequences', '/sequences'],
	['playlists', '/playlists'],
	['schedule', '/schedule'],
	['dj', '/dj'],
	['effects', '/effects'],
	['games', '/games'],
	['settings', '/settings'],
	['request', '/request'],
	['setup', '/setup?setup=1']
];
const viewports = [
	['desktop', { width: 1440, height: 900 }],
	['phone', { width: 390, height: 844 }]
];

const browser = await chromium.launch();
for (const [vname, vp] of viewports) {
	for (const [name, path] of pages) {
		if (filter && !name.includes(filter)) continue;
		const ctx = await browser.newContext({ viewport: vp, deviceScaleFactor: vname === 'phone' ? 2 : 1, colorScheme: 'dark' });
		const page = await ctx.newPage();
		const errors = [];
		page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
		page.on('pageerror', (e) => errors.push(e.message));
		const sep = path.includes('?') ? '&' : '?';
		await page.goto(`${base}${path}${sep}mock=1`, { waitUntil: 'networkidle' });
		await page.waitForTimeout(1600);
		await page.screenshot({ path: `${out}/${name}-${vname}.png`, fullPage: process.env.FULL === '1' });
		if (errors.length) console.log(`[${name}/${vname}] errors:`, errors.slice(0, 5));
		await ctx.close();
	}
}
await browser.close();
console.log('done');

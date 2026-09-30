import { chromium } from '@playwright/test';
import { mkdirSync } from 'node:fs';
const base = process.argv[2];
const out = process.argv[3];
const mock = process.argv[4] === 'mock';
mkdirSync(out, { recursive: true });
const pages = [
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
	'/reports',
	'/map',
	'/calibrate',
	'/yard-sign',
	'/settings',
	'/settings/features',
	'/settings/https',
	'/settings/power',
	'/settings/remote',
	'/settings/reports',
	'/settings/seasons',
	'/settings/sensors',
	'/settings/triggers',
	'/settings/updates',
	'/settings/xlights',
	'/request',
	'/trust',
	'/setup?setup=1'
];
const vps = [
	['desktop', { width: 1440, height: 900 }],
	['phone', { width: 390, height: 844 }]
];
const schemes = (process.env.SCHEMES ?? 'dark,light').split(',');
const browser = await chromium.launch();
for (const scheme of schemes)
	for (const [vn, vp] of vps) {
		const ctx = await browser.newContext({ viewport: vp, deviceScaleFactor: 1, colorScheme: scheme });
		for (const p of pages) {
			const page = await ctx.newPage();
			const errors = [];
			page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
			page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
			const sep = p.includes('?') ? '&' : '?';
			try {
				await page.goto(`${base}${p}${mock ? sep + 'mock=1' : ''}`, {
					waitUntil: 'networkidle',
					timeout: 20000
				});
			} catch (e) {
				errors.push('goto ' + e.message.split('\n')[0]);
			}
			await page.waitForTimeout(1200);
			const hs = await page
				.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
				.catch(() => 0);
			if (hs > 1) errors.push(`horizontal overflow ${hs}px`);
			const name = p === '/' ? 'dashboard' : p.slice(1).replace(/[/?=]/g, '_');
			await page.screenshot({ path: `${out}/${scheme}-${vn}-${name}.png`, fullPage: true });
			if (errors.length) console.log(`[${scheme}/${vn}${p}]`, errors.slice(0, 4).join(' | '));
			await page.close();
		}
		await ctx.close();
	}
await browser.close();
console.log('done');

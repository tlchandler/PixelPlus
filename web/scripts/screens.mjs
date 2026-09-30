// Capture full-page screenshots of every page, desktop and phone, and report console errors
// and sideways scrolling on the way.
// Usage: node scripts/screens.mjs [baseUrl] [outDir] [filter]
//   baseUrl  default http://localhost:4173 (demo backend via ?mock=1)
//   MOCK=0   use the real daemon behind baseUrl instead of the demo backend
//   SCHEMES  colour schemes, default "dark" (e.g. SCHEMES=dark,light)
//   VP       only one viewport: desktop | phone
import { chromium } from '@playwright/test';
import { mkdirSync } from 'node:fs';

const base = process.argv[2] ?? 'http://localhost:4173';
const out = process.argv[3] ?? 'screens';
const filter = process.argv[4];
const mock = process.env.MOCK !== '0';
const schemes = (process.env.SCHEMES ?? 'dark').split(',');
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
	['reports', '/reports'],
	['map', '/map'],
	['calibrate', '/calibrate'],
	['yard-sign', '/yard-sign'],
	['settings', '/settings'],
	['settings-features', '/settings/features'],
	['settings-https', '/settings/https'],
	['settings-power', '/settings/power'],
	['settings-remote', '/settings/remote'],
	['settings-reports', '/settings/reports'],
	['settings-seasons', '/settings/seasons'],
	['settings-sensors', '/settings/sensors'],
	['settings-triggers', '/settings/triggers'],
	['settings-updates', '/settings/updates'],
	['settings-xlights', '/settings/xlights'],
	['request', '/request'],
	['trust', '/trust'],
	['setup', '/setup?setup=1']
];
const viewports = [
	['desktop', { width: 1440, height: 900 }],
	['phone', { width: 390, height: 844 }]
];

const browser = await chromium.launch();
let problems = 0;
for (const scheme of schemes) {
	for (const [vname, vp] of viewports.filter(([n]) => !process.env.VP || process.env.VP === n)) {
		const ctx = await browser.newContext({
			viewport: vp,
			deviceScaleFactor: vname === 'phone' ? 2 : 1,
			colorScheme: scheme
		});
		for (const [name, path] of pages) {
			if (filter && !name.includes(filter)) continue;
			const page = await ctx.newPage();
			const errors = [];
			page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
			page.on('pageerror', (e) => errors.push(e.message));
			const sep = path.includes('?') ? '&' : '?';
			try {
				await page.goto(`${base}${path}${mock ? `${sep}mock=1` : ''}`, {
					waitUntil: 'networkidle',
					timeout: 20000
				});
			} catch (e) {
				errors.push(`load: ${e.message.split('\n')[0]}`);
			}
			await page.waitForTimeout(1600);
			const sideways = await page
				.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
				.catch(() => 0);
			if (sideways > 1) errors.push(`scrolls sideways by ${sideways}px`);
			await page.screenshot({ path: `${out}/${scheme}-${vname}-${name}.png`, fullPage: true });
			if (errors.length) {
				problems++;
				console.log(`[${scheme}/${vname} ${path}]`, errors.slice(0, 5).join(' | '));
			}
			await page.close();
		}
		await ctx.close();
	}
}
await browser.close();
console.log(problems ? `done, ${problems} page(s) with problems` : 'done');
process.exit(problems ? 1 : 0);

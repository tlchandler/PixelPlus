// WCAG AA contrast check computed straight from the design tokens in app.css, for both themes.
// Text must reach 4.5:1 on every surface it sits on; status colors are also used as text.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const css = readFileSync(fileURLToPath(new URL('../app.css', import.meta.url)), 'utf8');

function block(selectorStart: string): Record<string, string> {
	const i = css.indexOf(selectorStart);
	if (i < 0) throw new Error(`no ${selectorStart} block in app.css`);
	const body = css.slice(css.indexOf('{', i) + 1, css.indexOf('}', i));
	const out: Record<string, string> = {};
	for (const m of body.matchAll(/--([\w-]+):\s*([^;]+);/g)) out[m[1]] = m[2].trim();
	return out;
}

type RGBA = [number, number, number, number];
function parse(c: string): RGBA {
	const hex = /^#([0-9a-f]{6})$/i.exec(c);
	if (hex) {
		const n = parseInt(hex[1], 16);
		return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
	}
	const rgba = /^rgba?\(([^)]+)\)$/.exec(c);
	if (rgba) {
		const [r, g, b, a = '1'] = rgba[1].split(',').map((x) => x.trim());
		return [Number(r), Number(g), Number(b), Number(a)];
	}
	throw new Error(`can't parse color ${c}`);
}
const over = (fg: RGBA, bg: RGBA): RGBA => [
	fg[0] * fg[3] + bg[0] * (1 - fg[3]),
	fg[1] * fg[3] + bg[1] * (1 - fg[3]),
	fg[2] * fg[3] + bg[2] * (1 - fg[3]),
	1
];
function lum([r, g, b]: RGBA): number {
	const f = (v: number) => {
		v /= 255;
		return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
	};
	return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}
export function ratio(a: RGBA, b: RGBA): number {
	const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
	return (x + 0.05) / (y + 0.05);
}

const themes = {
	dark: block(":root,\n[data-theme='dark']"),
	light: block("[data-theme='light']")
};

for (const [name, t] of Object.entries(themes)) {
	const c = (k: string) => parse(t[k]);
	const surfaces = ['bg', 'sidebar', 'surface', 'surface-2', 'surface-3'];

	describe(`${name} theme contrast (WCAG AA)`, () => {
		for (const text of ['text', 'text-2', 'text-3', 'accent-text', 'green', 'red', 'blue', 'purple'])
			for (const s of surfaces)
				it(`--${text} on --${s} ≥ 4.5:1`, () => {
					expect(ratio(c(text), c(s))).toBeGreaterThanOrEqual(4.5);
				});

		// Badges and notices: colored text on its own soft tint, on a card or the page.
		for (const [fg, soft] of [
			['accent-text', 'accent-soft'],
			['green', 'green-soft'],
			['red', 'red-soft'],
			['blue', 'blue-soft'],
			['purple', 'purple-soft']
		])
			for (const s of ['bg', 'surface', 'surface-2'])
				it(`--${fg} on --${soft} over --${s} ≥ 4.5:1`, () => {
					expect(ratio(c(fg), over(c(soft), c(s)))).toBeGreaterThanOrEqual(4.5);
				});

		it('primary button text on the accent fill ≥ 4.5:1', () => {
			expect(ratio(c('accent-fg'), c('accent'))).toBeGreaterThanOrEqual(4.5);
		});
		for (const s of ['bg', 'surface', 'surface-2'])
			it(`accent fill against --${s} ≥ 3:1 (switches, progress, buttons)`, () => {
				expect(ratio(c('accent'), c(s))).toBeGreaterThanOrEqual(3);
			});
	});
}

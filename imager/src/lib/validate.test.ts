import { describe, expect, it } from 'vitest';
import { countryFromLocale, formatEta, normalizeHostname, slugHostname, validate } from './validate';
import { emptySettings } from './types';

describe('validate', () => {
	it('accepts defaults', () => {
		expect(validate(emptySettings({ country: 'US', timezone: 'America/Chicago' }))).toEqual([]);
	});

	it('mirrors the Rust rules', () => {
		const s = { ...emptySettings(), wifiSsid: 'x', wifiPassword: 'short', hostname: '-no', ssh: true };
		expect(validate(s).map((e) => e.field)).toEqual(['wifiPassword', 'wifiCountry', 'hostname', 'sshPassword']);
	});

	it('needs the 6-character web password the Pi requires', () => {
		const s = emptySettings({ country: 'US', timezone: 'America/Chicago' });
		expect(validate({ ...s, uiPassword: '12345' }).map((e) => e.field)).toEqual(['uiPassword']);
		expect(validate({ ...s, uiPassword: '123456' })).toEqual([]);
	});

	it('accepts hex PSKs and unicode SSIDs', () => {
		const s = { ...emptySettings({ country: 'GB' }), wifiSsid: 'Café ✨', wifiPassword: 'ab'.repeat(32) };
		expect(validate(s)).toEqual([]);
		expect(validate({ ...s, wifiSsid: '✨'.repeat(11) })[0].field).toBe('wifiSsid'); // 33 bytes
	});
});

describe('helpers', () => {
	it('hostnames', () => {
		expect(normalizeHostname(' PixelPlus-Garage.local ')).toBe('pixelplus-garage');
		expect(slugHostname('Garage Tree! (left)')).toBe('garage-tree-left');
		expect(slugHostname('Crème brûlée')).toBe('creme-brulee');
	});
	it('country from locale', () => {
		expect(countryFromLocale('en-US')).toBe('US');
		expect(countryFromLocale('de_DE.UTF-8')).toBe('DE');
		expect(countryFromLocale('fr')).toBe('');
	});
	it('eta', () => {
		expect(formatEta(30)).toBe('30 s left');
		expect(formatEta(125)).toBe('3 min left');
		expect(formatEta(Infinity)).toBe('');
	});
});

import { describe, expect, it } from 'vitest';
import { safeNext, secureOptions } from './secure';
import type { TlsStatus } from '$lib/api/types';

const st: TlsStatus = {
	enabled: true,
	active: true,
	port: 443,
	caFingerprint: 'AB:CD',
	caSubject: 'PixelPlus Local CA',
	leafNames: ['pixelplus.local', 'pixelplus', '192.168.1.20'],
	leafNotAfter: '2027-10-01T00:00:00Z',
	urls: { lan: ['https://192.168.1.20', 'https://pixelplus.local'] },
	secureNow: false,
	role: 'leader'
};
const loc = (hostname: string) => ({ hostname, pathname: '/calibrate', search: '?x=1', hash: '' });

describe('secure options', () => {
	it('keeps the address the phone already uses, with the same path', () => {
		const r = secureOptions(st, null, loc('pixelplus.local'));
		expect(r.problem).toBeNull();
		expect(r.options).toHaveLength(1);
		expect(r.options[0].url).toBe('https://pixelplus.local/calibrate?x=1');
		expect(r.options[0].trustUrl).toBe('/trust?next=%2Fcalibrate%3Fx%3D1');
	});

	it('falls back to the first LAN address, adds the port, orders Tailscale → LAN → tunnel', () => {
		const r = secureOptions(
			{
				...st,
				port: 8443,
				urls: {
					lan: ['https://192.168.1.20:8443'],
					tailscale: 'https://pp.tail1.ts.net',
					tunnel: 'https://lights.example.com'
				}
			},
			null,
			loc('some-other-name')
		);
		expect(r.options.map((o) => o.kind)).toEqual(['tailscale', 'lan', 'tunnel']);
		expect(r.options[1].url).toBe('https://192.168.1.20:8443/calibrate?x=1');
		expect(r.options[0].url).toBe('https://pp.tail1.ts.net/calibrate?x=1');
		const byIp = secureOptions({ ...st, port: 8443 }, null, loc('192.168.1.20'));
		expect(byIp.options[0].url).toBe('https://192.168.1.20:8443/calibrate?x=1');
	});

	it('explains why there is no secure page', () => {
		expect(secureOptions({ ...st, enabled: false }, null, loc('x')).problem).toBe('off');
		expect(secureOptions({ ...st, role: 'follower' }, null, loc('x')).problem).toBe('follower');
		expect(secureOptions({ ...st, caFingerprint: '' }, null, loc('x')).problem).toBe('none');
		expect(secureOptions(null, null, loc('x')).problem).toBe('none');
		const pub = {
			available: true,
			role: 'leader',
			port: 443,
			caFingerprint: 'AB',
			caSubject: 'x',
			leafNames: [],
			urls: ['https://10.0.0.5'],
			secureNow: false
		};
		expect(secureOptions(null, pub, loc('x')).options[0].url).toBe('https://10.0.0.5/calibrate?x=1');
		expect(secureOptions(null, { ...pub, available: false }, loc('x')).problem).toBe('off');
		// Tailscale still works when HTTPS on the LAN is off.
		const ts = secureOptions(
			{ ...st, enabled: false, urls: { lan: [], tailscale: 'https://a.ts.net' } },
			null,
			loc('x')
		);
		expect(ts.options.map((o) => o.kind)).toEqual(['tailscale']);
	});

	it('only returns to same-site paths', () => {
		expect(safeNext('/map')).toBe('/map');
		expect(safeNext('//evil.com')).toBe('/calibrate');
		expect(safeNext('https://evil.com')).toBe('/calibrate');
		expect(safeNext('/\\evil.com')).toBe('/calibrate');
		expect(safeNext(null, '/')).toBe('/');
	});
});

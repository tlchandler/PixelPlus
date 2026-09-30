// WS1 (F1): HTTPS / local CA status, trust downloads.
import type { TlsStatus } from '$lib/api/types';
import type { FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	let fingerprint =
		'A1:B2:C3:D4:E5:F6:07:18:29:3A:4B:5C:6D:7E:8F:90:A1:B2:C3:D4:E5:F6:07:18:29:3A:4B:5C:6D:7E:8F:90';
	let issued = new Date(Date.now() - 12 * 86400e3).toISOString();
	const created = new Date(Date.now() - 40 * 86400e3).toISOString();
	const status = (): TlsStatus => {
		const ips = ctx.server.system.ips.filter((ip) => !ip.includes(':'));
		const https = ctx.server.show.settings.https ?? { enabled: true };
		const extra = https.extraNames ?? [];
		const ok = (n: string) => /\.(local|lan|home\.arpa|internal)$/.test(n);
		const remote = ctx.server.show.settings.remote;
		const ts = remote?.tailscale;
		return {
			enabled: https.enabled,
			port: 443,
			active: https.enabled,
			listening: https.enabled,
			error: null,
			role: 'leader',
			caFingerprint: fingerprint,
			caSubject: `PixelPlus Local CA – ${ctx.server.show.name} – 3f2a`,
			caCreatedAt: created,
			caNotAfter: new Date(Date.parse(created) + 3650 * 86400e3).toISOString(),
			leafNames: [
				`${ctx.server.system.hostname}.local`,
				ctx.server.system.hostname,
				'localhost',
				...extra.filter(ok),
				...ips
			],
			leafNotAfter: new Date(Date.parse(issued) + 397 * 86400e3).toISOString(),
			leafIssuedAt: issued,
			rejectedNames: extra.filter((n) => !ok(n)),
			urls: {
				lan: [...ips.map((ip) => `https://${ip}`), `https://${ctx.server.system.hostname}.local`],
				...(ts?.enabled && ts.serveAdmin && ts.dnsName ? { tailscale: `https://${ts.dnsName}` } : {}),
				...(remote?.cloudflare?.adminHost ? { tunnel: `https://${remote.cloudflare.adminHost}` } : {})
			},
			secureNow: typeof window !== 'undefined' && window.isSecureContext && location.protocol === 'https:'
		};
	};
	ctx.route('GET', '/tls/status', () => status());
	ctx.route('POST', '/tls/rotate', ({ body }) => {
		if (body?.ca) fingerprint = fingerprint.split(':').reverse().join(':');
		issued = new Date().toISOString();
		return status();
	});
	ctx.route('GET', '/public/tls', () => {
		const s = status();
		return {
			available: s.enabled,
			role: 'leader',
			port: s.port,
			caFingerprint: s.caFingerprint,
			caSubject: s.caSubject,
			leafNames: s.leafNames,
			urls: s.urls.lan,
			secureNow: s.secureNow
		};
	});
	ctx.route(
		'GET',
		'/public/ca.crt',
		() =>
			new Blob([new Uint8Array([0x30, 0x82, 0x01, 0x00])], {
				type: 'application/x-x509-ca-cert'
			})
	);
	ctx.route(
		'GET',
		'/public/ca.mobileconfig',
		() =>
			new Blob(['<?xml version="1.0"?><plist version="1.0"><dict/></plist>'], {
				type: 'application/x-apple-aspen-config'
			})
	);
}

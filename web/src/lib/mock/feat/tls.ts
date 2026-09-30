// WS1 (F1): HTTPS / local CA status.
import type { TlsStatus } from '$lib/api/types';
import type { FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	let fingerprint =
		'A1:B2:C3:D4:E5:F6:07:18:29:3A:4B:5C:6D:7E:8F:90:A1:B2:C3:D4:E5:F6:07:18:29:3A:4B:5C:6D:7E:8F:90';
	ctx.route('GET', '/tls/status', (): TlsStatus => {
		const ips = ctx.server.system.ips.filter((ip) => !ip.includes(':'));
		return {
			enabled: ctx.server.show.settings.https?.enabled ?? true,
			port: 443,
			caFingerprint: fingerprint,
			caSubject: `PixelPlus Local CA – ${ctx.server.show.name} – 3f2a`,
			leafNames: [`${ctx.server.system.hostname}.local`, ctx.server.system.hostname, ...ips],
			leafNotAfter: new Date(Date.now() + 380 * 86400e3).toISOString(),
			urls: { lan: ips.map((ip) => `https://${ip}`) },
			secureNow: typeof window !== 'undefined' && window.isSecureContext
		};
	});
	ctx.route('POST', '/tls/rotate', ({ body }) => {
		if (body?.ca) fingerprint = fingerprint.split(':').reverse().join(':');
		return { ok: true };
	});
	ctx.route(
		'GET',
		'/public/ca.crt',
		() =>
			new Blob(['-----BEGIN CERTIFICATE-----\nZGVtbw==\n-----END CERTIFICATE-----\n'], {
				type: 'application/x-x509-ca-cert'
			})
	);
}

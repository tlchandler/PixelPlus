/** Where to send a phone that is on an insecure (http) page (see SecureGate.svelte). */
import type { TlsStatus } from '$lib/api/types';
import type { PublicTls } from './api';

export interface SecureOption {
	kind: 'tailscale' | 'lan' | 'tunnel';
	url: string;
	title: string;
	detail: string;
	/** For `lan`: the trust page (on this http address) that returns here afterwards. */
	trustUrl?: string;
}

export interface SecureChoice {
	options: SecureOption[];
	/** Why there's no LAN option: HTTPS off, a follower, or no certificate yet. */
	problem: 'off' | 'follower' | 'none' | null;
}

type Loc = Pick<Location, 'hostname' | 'pathname' | 'search' | 'hash'>;

function hostPort(host: string, port: number): string {
	const h = host.includes(':') && !host.startsWith('[') ? `[${host}]` : host;
	return port === 443 || !port ? h : `${h}:${port}`;
}

/** A same-site path to return to (never another site). */
export function safeNext(next: string | null | undefined, fallback = '/calibrate'): string {
	if (!next || !next.startsWith('/') || next.startsWith('//') || next.includes('\\')) return fallback;
	return next;
}

export function secureOptions(st: TlsStatus | null, pub: PublicTls | null, loc: Loc): SecureChoice {
	const path = loc.pathname + loc.search + loc.hash;
	let problem: SecureChoice['problem'] = null;
	const role = st?.role ?? pub?.role;
	if (role === 'follower') problem = 'follower';
	else if (st) {
		if (!st.enabled || st.active === false) problem = 'off';
		else if (!st.caFingerprint) problem = 'none';
	} else if (pub) {
		if (!pub.caFingerprint) problem = 'none';
		else if (!pub.available) problem = 'off';
	} else problem = 'none';

	const options: SecureOption[] = [];
	const ts = st?.urls.tailscale;
	if (ts)
		options.push({
			kind: 'tailscale',
			url: ts.replace(/\/$/, '') + path,
			title: 'Open through Tailscale',
			detail: ts.replace(/^https:\/\//, '')
		});
	if (!problem) {
		const port = st?.port ?? pub?.port ?? 443;
		const names = st?.leafNames ?? pub?.leafNames ?? [];
		const lan = st?.urls.lan ?? pub?.urls ?? [];
		const here = loc.hostname.replace(/^\[|\]$/g, '').toLowerCase();
		const base = names.includes(here) ? `https://${hostPort(here, port)}` : lan[0];
		if (base)
			options.push({
				kind: 'lan',
				url: base.replace(/\/$/, '') + path,
				title: 'Open the secure page',
				detail: base.replace(/^https:\/\//, ''),
				trustUrl: `/trust?next=${encodeURIComponent(path)}`
			});
	}
	const tunnel = st?.urls.tunnel;
	if (tunnel)
		options.push({
			kind: 'tunnel',
			url: tunnel.replace(/\/$/, '') + path,
			title: 'Open through your Cloudflare address',
			detail: tunnel.replace(/^https:\/\//, '')
		});
	return { options, problem };
}

// Secret trigger links (ARCHITECTURE §12.18; mirrors api/hooks.rs): make / rotate / revoke a
// trigger's token, the addresses to use and each link's last use, and the link itself.
import type { TriggerLinkUse, TriggerLinks } from '$lib/api/types';
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';

function token(): string {
	const bytes = new Uint8Array(32);
	globalThis.crypto.getRandomValues(bytes);
	const b64 = btoa(String.fromCharCode(...bytes))
		.replace(/\+/g, '-')
		.replace(/\//g, '_')
		.replace(/=+$/, '');
	return `ppt_${b64}`;
}

export function register(ctx: FeatureContext) {
	/** The demo keeps the tokens themselves (the daemon keeps only their SHA-256). */
	const tokens = new Map<string, string>();
	const uses: Record<string, TriggerLinkUse> = {
		trhass0001: {
			at: new Date(Date.now() - 42 * 60_000).toISOString(),
			from: '192.168.1.20',
			origin: 'home',
			fired: true,
			message: 'Stopped the show'
		}
	};
	const httpTrigger = (id: string) => {
		const t = ctx.server.show.settings.triggers.find((x) => x.id === id);
		if (!t) throw new HttpError(404, 'not_found', 'That trigger was not found');
		if (t.kind !== 'http')
			throw new HttpError(400, 'bad_request', 'Only web-link triggers have a secret link.');
		return t;
	};

	ctx.route('POST', '/triggers/([^/]+)/token', ({ params }) => {
		const t = httpTrigger(params[0]);
		const rotated = !!t.tokenHint;
		const tok = token();
		tokens.set(t.id, tok);
		t.tokenHint = tok.slice(-4);
		t.tokenCreatedAt = nowIso();
		delete uses[t.id];
		ctx.bump();
		return {
			token: tok,
			tokenHint: t.tokenHint,
			tokenCreatedAt: t.tokenCreatedAt,
			path: `/api/v1/hooks/trigger/${t.id}`,
			rotated
		};
	});
	ctx.route('DELETE', '/triggers/([^/]+)/token', ({ params }) => {
		const t = ctx.server.show.settings.triggers.find((x) => x.id === params[0]);
		if (!t) throw new HttpError(404, 'not_found', 'That trigger was not found');
		delete t.tokenHint;
		delete t.tokenCreatedAt;
		tokens.delete(t.id);
		delete uses[t.id];
		ctx.bump();
		return { ok: true };
	});
	ctx.route('GET', '/triggers/links', (): TriggerLinks => {
		const s = ctx.server.show.settings;
		const addresses: TriggerLinks['addresses'] = [
			{ kind: 'name', label: 'pixelplus.local', base: 'http://pixelplus.local' },
			{ kind: 'ip', label: '192.168.1.50', base: 'http://192.168.1.50' }
		];
		if (s.https?.enabled)
			addresses.push({ kind: 'https', label: 'pixelplus.local (HTTPS)', base: 'https://pixelplus.local' });
		const host = s.remote?.cloudflare?.publicHost;
		if (s.remote?.publicListener && host)
			addresses.push({ kind: 'internet', label: 'From the internet', base: `https://${host}` });
		return { addresses, links: uses };
	});
	// The link itself (what Home Assistant calls): token in the header or the query.
	const hook = (method: string) => (c: { params: string[]; query: URLSearchParams; body: unknown }) => {
		const t = ctx.server.show.settings.triggers.find((x) => x.id === c.params[0] && x.kind === 'http');
		if (!t) throw new HttpError(404, 'not_found', 'There is no trigger link here.');
		const given = c.query.get('token') ?? (c.body as { token?: string } | null)?.token;
		if (!given) throw new HttpError(401, 'token_required', 'Send the trigger’s token.');
		if (!t.tokenHint || (given !== tokens.get(t.id) && !given.endsWith(t.tokenHint)))
			throw new HttpError(
				401,
				'bad_token',
				'That token isn’t right (the link may have been renewed or turned off).'
			);
		if (method === 'GET' && !t.allowGet)
			throw new HttpError(405, 'get_not_allowed', 'Simple GET links are off for this trigger.');
		uses[t.id] = {
			at: nowIso(),
			from: '192.168.1.20',
			origin: 'home',
			fired: true,
			message: `Ran "${t.name}"`
		};
		return { ok: true, fired: true, message: `Ran "${t.name}"` };
	};
	ctx.route('POST', '/hooks/trigger/([^/]+)', hook('POST'));
	ctx.route('GET', '/hooks/trigger/([^/]+)', hook('GET'));
}

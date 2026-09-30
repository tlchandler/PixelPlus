// WS6 (F8 seasons + F16 xLights admin): mirrors api/profiles.rs and the
// /xlights/* admin endpoints the daemon merges through it.
import type { ShowProfile } from '$lib/api/types';
import type { ActiveSeason, UploadEntry, XlightsStatus } from '$lib/insight/api';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError } from './context';

export function register(ctx: FeatureContext) {
	const show = () => ctx.server.show;
	const list = () => (show().profiles ??= []);
	const find = (id: string) => {
		const p = list().find((x) => x.id === id);
		if (!p) throw new HttpError(404, 'not_found', 'That season was not found');
		return p;
	};
	/** Save the live season fields into the active profile. */
	const storeLive = (p: ShowProfile) => {
		const s = show();
		p.schedule = structuredClone(s.schedule);
		p.requestsPlaylistId = s.settings.requests.playlistId;
		p.requestsMessage = s.settings.requests.message;
		p.gamesEnabled = s.settings.games.enabled;
		if (s.settings.power)
			p.power = { dim: structuredClone(s.settings.power.dim), maxBrightness: s.settings.power.maxBrightness };
	};
	const apply = (p: ShowProfile) => {
		const s = show();
		const location = s.schedule.location;
		s.schedule = { ...structuredClone(p.schedule), location };
		s.settings.requests.playlistId = p.requestsPlaylistId;
		if (p.requestsMessage) s.settings.requests.message = p.requestsMessage;
		if (p.gamesEnabled !== undefined) s.settings.games.enabled = p.gamesEnabled;
		s.activeProfileId = p.id;
	};
	const inRange = (md: string, r?: { start: string; end: string }) =>
		!!r && (r.start <= r.end ? md >= r.start && md <= r.end : md >= r.start || md <= r.end);

	ctx.route('GET', '/profiles/active', (): ActiveSeason => {
		const a = list().find((p) => p.id === show().activeProfileId);
		const md = new Date().toISOString().slice(5, 10);
		return {
			id: a?.id ?? null,
			name: a?.name ?? null,
			icon: a?.icon ?? null,
			color: a?.color ?? null,
			autoSwitch: !!show().profileAutoSwitch,
			scheduledId: list().find((p) => inRange(md, p.dateRange))?.id ?? null,
			nextSwitch: null
		};
	});
	ctx.route('PUT', '/profiles/auto-switch', ({ body }) => {
		if (body?.enabled && !list().some((p) => p.dateRange))
			throw new HttpError(400, 'bad_request', 'Give at least one season its dates first.');
		show().profileAutoSwitch = !!body?.enabled;
		ctx.bump();
		return { enabled: !!body?.enabled };
	});
	ctx.route('POST', '/profiles/capture', ({ body }) => {
		const p: ShowProfile = {
			id: newId(),
			name: body?.name || 'New season',
			priority: 0,
			schedule: structuredClone(show().schedule)
		};
		storeLive(p);
		list().push(p);
		if (!show().activeProfileId && list().length === 1) show().activeProfileId = p.id;
		ctx.bump();
		return p;
	});
	ctx.route('GET', '/profiles/preview-switch/([^/]+)', ({ params }) => {
		const p = find(params[0]);
		const s = show();
		const lines = [`Schedule: ${s.schedule.entries.length} show times → ${p.schedule.entries.length}`];
		const eff = (id?: string) => s.effects.find((e) => e.id === id)?.name ?? 'none';
		if (s.schedule.idleEffectId !== p.schedule.idleEffectId)
			lines.push(`Idle look: ${eff(s.schedule.idleEffectId)} → ${eff(p.schedule.idleEffectId)}`);
		if (p.disabledPropIds?.length) lines.push(`Props kept dark: ${p.disabledPropIds.length}`);
		return { lines };
	});
	ctx.route('POST', '/profiles/([^/]+)/activate', ({ params, body }) => {
		const p = find(params[0]);
		const prev = list().find((x) => x.id === show().activeProfileId);
		if (prev && prev.id !== p.id && body?.saveCurrent !== false) storeLive(prev);
		apply(p);
		ctx.log('info', `Season switched to ${p.name}`);
		ctx.bump();
		return show();
	});
	ctx.route('POST', '/profiles', ({ body }) => {
		const p: ShowProfile = {
			priority: 0,
			schedule: { enabled: true, location: show().schedule.location, entries: [] },
			...body,
			id: newId(),
			name: (body?.name || 'New season').trim()
		};
		list().push(p);
		ctx.bump();
		return p;
	});
	ctx.route('GET', '/profiles', () => list());
	ctx.route('GET', '/profiles/([^/]+)', ({ params }) => find(params[0]));
	ctx.route('PUT', '/profiles/([^/]+)', ({ params, body }) => {
		const p = find(params[0]);
		for (const [k, v] of Object.entries(body ?? {})) {
			if (k === 'id') continue;
			if (v === null) delete (p as any)[k];
			else (p as any)[k] = v;
		}
		if (p.id === show().activeProfileId) apply(p);
		ctx.bump();
		return p;
	});
	ctx.route('DELETE', '/profiles/([^/]+)', ({ params }) => {
		const arr = list();
		const i = arr.findIndex((x) => x.id === params[0]);
		if (i < 0) throw new HttpError(404, 'not_found', 'That season was not found');
		arr.splice(i, 1);
		if (show().activeProfileId === params[0]) delete show().activeProfileId;
		ctx.bump();
		return { ok: true };
	});

	// ---- F16 xLights admin (/xlights/*)
	let uploads: UploadEntry[] = [
		{
			at: new Date(Date.now() - 36e5).toISOString(),
			name: 'Wizards in Winter.fseq',
			kind: 'sequence',
			ok: true,
			message: 'Updated "Wizards in Winter" with its song',
			bytes: 18_400_000,
			source: 'xlights',
			replaced: true
		}
	];
	ctx.route('GET', '/xlights/status', (): XlightsStatus => {
		const x = show().settings.xlights ?? { fppConnect: false, addToPlaylists: true };
		const passwordSet = x.passwordHash !== undefined;
		const adminPasswordSet = show().settings.security.passwordHash !== undefined;
		const reason = !x.fppConnect
			? 'Turned off.'
			: adminPasswordSet && !passwordSet
				? 'Set an upload password: this show has a sign-in password.'
				: undefined;
		return {
			enabled: x.fppConnect,
			passwordSet,
			adminPasswordSet,
			ready: !reason,
			reason,
			addresses: ['192.168.1.40'],
			hostname: 'pixelplus',
			uploads,
			watch: {
				folder: x.watchFolder ?? null,
				exists: !!x.watchFolder,
				suggested: '/var/lib/pixelplus/xlights-drop',
				lastScan: x.watchFolder ? new Date().toISOString() : undefined
			}
		};
	});
	ctx.route('PUT', '/xlights/password', ({ body }) => {
		const pw: string = body?.password ?? '';
		if (pw && pw.length < 6)
			throw new HttpError(400, 'bad_request', 'Use at least 6 characters for the upload password.');
		const x = (show().settings.xlights ??= { fppConnect: false, addToPlaylists: true });
		if (pw) x.passwordHash = '';
		else delete x.passwordHash;
		ctx.bump();
		return { passwordSet: !!pw };
	});
	ctx.route('DELETE', '/xlights/uploads', () => {
		uploads = [];
		return { ok: true };
	});
}

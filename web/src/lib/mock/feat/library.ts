// WS2 (F18): tags, smart playlist previews and play history, for demo mode. The smart
// expansion here is a simplified version of the daemon's `smartlist::expand`.
import type { PlaylistItem, Show, SmartRules } from '$lib/api/types';
import type { FeatureContext } from './context';
import { HttpError } from './context';
import { normalizeTag } from '$lib/library/tags';

function seeded(seed: number) {
	let s = seed >>> 0 || 1;
	return () => {
		s = (s * 1664525 + 1013904223) >>> 0;
		return s / 2 ** 32;
	};
}

function durOf(show: Show, it: PlaylistItem): number {
	switch (it.type) {
		case 'sequence':
			return show.sequences.find((s) => s.id === it.sequenceId)?.durationMs ?? 0;
		case 'media':
			return show.media.find((m) => m.id === it.mediaId)?.durationMs ?? 0;
		case 'dj': {
			const c = show.djClips.find((x) => x.id === it.djClipId);
			return show.media.find((m) => m.id === c?.mediaId)?.durationMs ?? 0;
		}
		case 'effect':
		case 'pause':
		case 'countdown':
			return it.durationMs;
		default:
			return 0;
	}
}

export function expandDemo(show: Show, r: SmartRules, startMs: number, seed: number) {
	const rnd = seeded(seed);
	const notes: string[] = [];
	let c = show.sequences.filter(
		(s) =>
			(!r.includeTags.length ||
				(r.includeMode === 'all'
					? r.includeTags.every((t) => s.tags?.includes(t))
					: r.includeTags.some((t) => s.tags?.includes(t)))) &&
			!r.excludeTags.some((t) => s.tags?.includes(t)) &&
			(!r.maxItemMs || s.durationMs <= r.maxItemMs)
	);
	if (!c.length)
		notes.push(
			r.includeTags.length
				? `No songs are tagged “${r.includeTags.join('” or “')}”. Tag some songs on the Sequences page.`
				: 'No songs match these rules yet. Upload sequences or loosen the rules.'
		);
	if (r.order === 'shuffle' || r.order === 'leastRecent') c = [...c].sort(() => rnd() - 0.5);
	if (r.order === 'rotation') c = [...c].sort((a, b) => a.name.localeCompare(b.name));
	const picked: PlaylistItem[] = [];
	const pin = (l: PlaylistItem[]) => l.reduce((a, it) => a + durOf(show, it), 0);
	let total = pin(r.pinnedFirst) + pin(r.pinnedLast);
	for (const s of c) {
		if (r.targetDurationMs && total + s.durationMs > r.targetDurationMs + s.durationMs / 2) break;
		if (
			r.interleave.length &&
			r.interleaveEvery &&
			picked.length &&
			picked.length % r.interleaveEvery === 0
		) {
			const it = { ...r.interleave[0], id: `${r.interleave[0].id}-si${picked.length}` } as PlaylistItem;
			picked.push(it);
			total += durOf(show, it);
		}
		picked.push({ id: `sm${picked.length}-${s.id}`, type: 'sequence', sequenceId: s.id });
		total += s.durationMs;
	}
	const items = [...r.pinnedFirst, ...picked, ...r.pinnedLast];
	let t = startMs;
	const startsAt = items.map((it) => {
		const at = new Date(t).toISOString();
		t += durOf(show, it);
		return at;
	});
	return { items, totalMs: items.reduce((a, it) => a + durOf(show, it), 0), notes, startsAt, seed };
}

function startOf(query: { date?: string; start?: string }): number {
	const base = query.date ? new Date(`${query.date}T${query.start || '18:00'}:00`) : new Date();
	return Number.isNaN(base.getTime()) ? Date.now() : base.getTime();
}

export function register(ctx: FeatureContext) {
	const show = () => ctx.server.show;
	ctx.route('POST', '/sequences/tags', ({ body }) => {
		const ids: string[] = body?.ids ?? [];
		if (!ids.length) throw new HttpError(400, 'bad_request', 'Pick at least one song.');
		const add = (body.add ?? []).map(normalizeTag).filter(Boolean);
		const remove = (body.remove ?? []).map(normalizeTag);
		for (const x of [...show().sequences, ...show().media].filter((x) => ids.includes(x.id))) {
			const t = (x.tags ?? []).filter((y) => !remove.includes(y));
			for (const a of add) if (!t.includes(a)) t.push(a);
			x.tags = t;
		}
		ctx.bump();
		return show().sequences;
	});
	ctx.route('GET', '/library/tags', () => {
		const m = new Map<string, { sequences: number; media: number }>();
		for (const s of show().sequences)
			for (const t of s.tags ?? [])
				m.set(t, { ...(m.get(t) ?? { sequences: 0, media: 0 }), sequences: (m.get(t)?.sequences ?? 0) + 1 });
		for (const s of show().media)
			for (const t of s.tags ?? [])
				m.set(t, { ...(m.get(t) ?? { sequences: 0, media: 0 }), media: (m.get(t)?.media ?? 0) + 1 });
		return [...m.entries()]
			.map(([name, c]) => ({ name, color: show().tagDefs?.find((d) => d.name === name)?.color, ...c }))
			.sort((a, b) => a.name.localeCompare(b.name));
	});
	ctx.route('PUT', '/library/tags/([^/]+)', ({ params, body }) => {
		const old = params[0];
		const name = body?.name ? normalizeTag(body.name) : old;
		for (const x of [...show().sequences, ...show().media])
			if (x.tags?.includes(old)) x.tags = [...new Set(x.tags.map((t) => (t === old ? name : t)))];
		const defs = (show().tagDefs ?? []).filter((d) => d.name !== old && d.name !== name);
		if (body?.color) defs.push({ name, color: body.color });
		show().tagDefs = defs;
		ctx.bump();
		return { ok: true };
	});
	ctx.route('DELETE', '/library/tags/([^/]+)', ({ params }) => {
		for (const x of [...show().sequences, ...show().media]) x.tags = x.tags?.filter((t) => t !== params[0]);
		show().tagDefs = show().tagDefs?.filter((d) => d.name !== params[0]);
		ctx.bump();
		return { ok: true };
	});
	ctx.route('GET', '/playlists/([^/]+)/preview', ({ params, query }) => {
		const pl = show().playlists.find((p) => p.id === params[0]);
		if (!pl) throw new HttpError(404, 'not_found', 'That playlist');
		const start = startOf({ date: query.get('date') ?? undefined, start: query.get('start') ?? undefined });
		if (!pl.smart) {
			const items = pl.items;
			return {
				items,
				totalMs: items.reduce((a, it) => a + durOf(show(), it), 0),
				notes: ['This playlist isn’t smart; these are its songs.']
			};
		}
		return expandDemo(show(), pl.smart, start, Number(query.get('seed') ?? 7));
	});
	ctx.route('POST', '/library/smart-preview', ({ body }) => {
		if (!body?.rules) throw new HttpError(400, 'bad_request', 'Send the rules to preview.');
		return expandDemo(show(), body.rules, startOf(body), Number(body.seed ?? 7));
	});
	ctx.route('GET', '/library/history', ({ query }) => {
		const days = Number(query.get('days') ?? 14);
		return show().sequences.map((s, i) => ({
			sequenceId: s.id,
			plays: (i * 7) % (days + 3),
			lastPlayed: new Date(Date.now() - (i + 1) * 86400e3).toISOString()
		}));
	});
}

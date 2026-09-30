// WS2 (F18): tags, smart playlist preview, play history.
import type { PlaylistItem, SmartPreview } from '$lib/api/types';
import type { FeatureContext } from './context';
import { HttpError } from './context';

export function register(ctx: FeatureContext) {
	const show = () => ctx.server.show;
	ctx.route('POST', '/sequences/tags', ({ body }) => {
		for (const s of show().sequences.filter((x) => (body?.ids ?? []).includes(x.id))) {
			const t = new Set(s.tags ?? []);
			for (const a of body.add ?? []) t.add(a);
			for (const r of body.remove ?? []) t.delete(r);
			s.tags = [...t];
		}
		ctx.bump();
		return show().sequences;
	});
	ctx.route('GET', '/playlists/([^/]+)/preview', ({ params }): SmartPreview => {
		const pl = show().playlists.find((p) => p.id === params[0]);
		if (!pl) throw new HttpError(404, 'not_found', 'That playlist');
		const rules = pl.smart;
		const seqs = show().sequences.filter(
			(s) => !rules?.includeTags.length || rules.includeTags.some((t) => s.tags?.includes(t))
		);
		const items: PlaylistItem[] = rules
			? seqs.map((s) => ({ id: `sm-${s.id}`, type: 'sequence' as const, sequenceId: s.id }))
			: pl.items;
		const totalMs = items.reduce(
			(a, it) =>
				a +
				(it.type === 'sequence'
					? (show().sequences.find((s) => s.id === it.sequenceId)?.durationMs ?? 0)
					: 0),
			0
		);
		return { items, totalMs, notes: rules ? [] : ['This playlist is not smart; showing its items.'] };
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

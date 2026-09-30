// WS3 (F12 power, F20 surprises): power supplies, live power, budgets, and the
// surprise try-it endpoints — shaped like pixelplusd's answers.
import type { PowerLive, Show } from '$lib/api/types';
import type { FeatureContext } from './context';
import { crud, HttpError } from './context';

type Group = {
	id: string;
	amps: number | null;
	budget: number;
	scale: number;
	tauMs: number;
	members: number[];
};

/** Budget groups per node, like `pixelplus_core::power::show_budgets` (simplified). */
function budgets(show: Show): Record<string, Group[]> {
	const p = show.settings.power;
	const out: Record<string, Group[]> = {};
	if (p?.mode === 'off') return out;
	const safety = p?.safety ?? 0.9;
	for (const n of show.nodes) out[n.id] = [];
	for (const r of show.receivers) {
		const fuse = r.fuseAmps ?? (r.kind === 'diffrx' || r.kind === 'diffsmart-rx' ? 6 : undefined);
		const ports = [1, 2, 3, 4].map((port) => ({ port, output: (r.jack - 1) * 4 + port }));
		if (fuse)
			for (const { port, output } of ports)
				out[r.nodeId]?.push({
					id: `port:${r.id}:${port}`,
					amps: 0,
					budget: fuse * 0.8 * safety,
					scale: 1,
					tauMs: 8000,
					members: [output]
				});
		if (r.mainFuseAmps)
			out[r.nodeId]?.push({
				id: `bus:${r.id}`,
				amps: 0,
				budget: r.mainFuseAmps * safety,
				scale: 1,
				tauMs: 1000,
				members: ports.map((x) => x.output)
			});
	}
	for (const s of show.powerSupplies ?? []) {
		const nodes = new Set([
			...s.receiverIds.map((id) => show.receivers.find((r) => r.id === id)?.nodeId).filter(Boolean),
			...s.directOutputs.map((d) => d.nodeId)
		] as string[]);
		for (const n of nodes)
			out[n]?.push({
				id: `supply:${s.id}`,
				amps: 0,
				budget: (s.amps * safety) / nodes.size,
				scale: 1,
				tauMs: 0,
				members: []
			});
	}
	return out;
}

export function register(ctx: FeatureContext) {
	crud(ctx, '/power-supplies', () => (ctx.server.show.powerSupplies ??= []));
	ctx.route('GET', '/power/live', (): PowerLive => {
		const show = ctx.server.show;
		const playing = ctx.server.play.state === 'playing';
		const b = budgets(show);
		return {
			nodes: show.nodes.map((n, i) => ({
				nodeId: n.id,
				mode: show.settings.power?.mode ?? 'warn',
				limiting: false,
				minScale: 1,
				groups: (b[n.id] ?? []).map((g) => ({
					id: g.id,
					amps:
						i === 0
							? Math.round((playing ? g.budget * (0.35 + Math.random() * 0.4) : 0.2) * 100) / 100
							: null,
					budget: Math.round(g.budget * 100) / 100,
					scale: 1
				}))
			})) as PowerLive['nodes']
		};
	});
	ctx.route('GET', '/power/budget', ({ query }) => {
		const all = budgets(ctx.server.show);
		const id = query.get('nodeId');
		const shape = (gs: Group[]) => ({
			mode: ctx.server.show.settings.power?.mode ?? 'warn',
			safety: ctx.server.show.settings.power?.safety ?? 0.9,
			groups: gs.map((g) => ({
				id: g.id,
				kind: g.id.split(':')[0],
				budgetA: g.budget,
				tauMs: g.tauMs,
				members: g.members
			})),
			mApp: {}
		});
		if (id) {
			if (!(id in all)) throw new HttpError(404, 'not_found', 'That controller was not found.');
			return shape(all[id]);
		}
		return Object.fromEntries(Object.entries(all).map(([k, v]) => [k, shape(v)]));
	});
	ctx.route('POST', '/player/surprise', ({ body }) => {
		if (!body?.ref) throw new HttpError(400, 'bad_request', 'Pick the sequence or look this surprise shows.');
		const props = body.target?.propIds?.length ?? ctx.server.show.props.length;
		ctx.log('info', `Surprise: ${body.ref}`);
		return {
			ok: true,
			surprise: { name: String(body.ref), durationMs: body.durationMs ?? 5000, props, replaced: false }
		};
	});
	ctx.route('POST', '/player/surprise/stop', () => ({ ok: true }));
}

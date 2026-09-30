// WS4 (F6): camera mapping runs.
import type { MapPlan, MappingRun, MappingRunStart } from '$lib/api/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';

export function register(ctx: FeatureContext) {
	const runs: MappingRun[] = [];
	const find = (id: string) => {
		const r = runs.find((x) => x.id === id);
		if (!r) throw new HttpError(404, 'not_found', 'That mapping run');
		return r;
	};
	ctx.route('POST', '/mapping/runs', ({ body }): MappingRunStart => {
		const show = ctx.server.show;
		const scope = body?.scope ?? { all: true };
		const outs = show.nodes.flatMap((n) =>
			n.outputs
				.filter((o) =>
					show.props.some((p) => p.segments.some((s) => s.nodeId === n.id && s.output === o.index))
				)
				.map((o) => ({ nodeId: n.id, output: o.index }))
		);
		const targets = outs
			.filter((t) => scope.all || !scope.nodeId || scope.nodeId === t.nodeId)
			.map((t, k) => ({ k, ...t, label: `${show.nodes.find((n) => n.id === t.nodeId)?.name} ${t.output}` }));
		const plan: MapPlan = {
			seed: Math.floor(Math.random() * 2 ** 31),
			bitMs: body?.bitMs ?? 200,
			level: body?.level ?? 77,
			passes: 3,
			phases: 3,
			targets: targets.map((t) => ({ nodeId: t.nodeId, output: t.output, maxPixels: 800 })),
			pixelBits: 11,
			startPosMs: 0
		};
		const run: MappingRun = { id: newId(), startedAt: nowIso(), scope, plan, targets };
		runs.unshift(run);
		const preambleMs = 2400;
		const phaseAms = 12 * plan.bitMs;
		const phaseBms = 2 * plan.pixelBits * plan.bitMs;
		return {
			runId: run.id,
			plan,
			schedule: { preambleMs, phaseAms, phaseBms, totalMs: (preambleMs + phaseAms + phaseBms) * plan.passes },
			codebook: targets.map((_, k) =>
				Array.from({ length: 12 }, (_, b) => ((k * 7 + b * 5) % 12 < 6 ? 1 : 0))
			)
		};
	});
	ctx.route('GET', '/mapping/runs', () => runs);
	ctx.route('GET', '/mapping/runs/([^/]+)', ({ params }) => find(params[0]));
	ctx.route('DELETE', '/mapping/runs/([^/]+)', ({ params }) => {
		find(params[0]);
		runs.splice(
			runs.findIndex((r) => r.id === params[0]),
			1
		);
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/stop', ({ params }) => {
		find(params[0]);
		ctx.broadcast('mapping', { runId: params[0], state: 'stopped', pct: 0 });
		return { ok: true };
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/results', ({ params, body }) => {
		const r = find(params[0]);
		r.results = { detected: body?.detected ?? [], proposals: body?.proposals ?? [] };
		return r;
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/apply', ({ params }) => {
		const r = find(params[0]);
		r.appliedSnapshotId = 'snap-map-' + r.id;
		ctx.bump();
		return { show: ctx.server.show, snapshotId: r.appliedSnapshotId };
	});
}

// WS4 (F6): camera mapping runs, mirroring the daemon (api/mapping.rs): plans
// from the real mapcode mirror, stored results, and proposals applied to the
// demo show (after a demo snapshot, so Undo works).
import type { Show } from '$lib/api/types';
import { codeBitsFor, codebook, PHASE_A } from '$lib/cv/mapcode';
import { buildPlan, scopeTargets, type Scope } from '$lib/cv/plan';
import type { CvProposal, MapStart, StoredRun } from '$lib/cv/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';

export const runs: StoredRun[] = [];

export function find(id: string): StoredRun {
	const r = runs.find((x) => x.id === id);
	if (!r) throw new HttpError(404, 'not_found', 'That mapping run was not found');
	return r;
}

export function startResponse(r: StoredRun, schedule: MapStart['schedule']): MapStart {
	const bits = r.plan.phases & PHASE_A ? codeBitsFor(r.plan.targets.length) : 0;
	return {
		runId: r.id,
		kind: r.kind ?? 'map',
		startedAt: r.startedAt,
		plan: r.plan,
		schedule,
		codebook: bits
			? r.plan.targets.map((_, k) =>
					Array.from({ length: bits }, (_, i) => (codebook(bits)[k] >> (bits - 1 - i)) & 1)
				)
			: [],
		targets: r.targets
	};
}

/** Apply stored proposals to the show (the daemon's `apply_proposals`). */
export function applyProposals(show: Show, proposals: CvProposal[]): string[] {
	const done: string[] = [];
	const seg = (d: { propId: string; segment?: number }) => {
		const s = show.props.find((p) => p.id === d.propId)?.segments[d.segment ?? 0];
		if (!s) throw new HttpError(400, 'bad_request', 'A proposal refers to a missing prop.');
		return s;
	};
	for (const p of proposals) {
		const d = (p.data ?? {}) as Record<string, any>;
		if (p.kind === 'swap') {
			const a = seg(d.a),
				b = seg(d.b);
			const keep = { nodeId: a.nodeId, output: a.output, startPixel: a.startPixel, nullPixels: a.nullPixels };
			Object.assign(a, {
				nodeId: b.nodeId,
				output: b.output,
				startPixel: b.startPixel,
				nullPixels: b.nullPixels
			});
			Object.assign(b, keep);
		} else if (p.kind === 'reverse') {
			const s = seg(d as { propId: string; segment?: number });
			s.reverse = !s.reverse;
		} else if (p.kind === 'layout') {
			const prop = show.props.find((x) => x.id === d.propId);
			if (prop) prop.layout = { ...d.layout, source: 'camera' };
		} else if (p.kind === 'pixelCount') {
			const node = show.nodes.find((n) => n.id === d.nodeId);
			const out = node?.outputs.find((o) => o.index === d.output);
			if (out) out.measuredPixels = { count: d.count, method: 'camera', at: nowIso(), dead: d.dead ?? [] };
		} else continue;
		done.push(p.message);
	}
	return done;
}

export function register(ctx: FeatureContext) {
	ctx.route('POST', '/mapping/runs', ({ body }): MapStart => {
		const show = ctx.server.show;
		const scope: Scope = body?.scope ?? { all: true };
		if (scope.nodeId && !show.nodes.some((n) => n.id === scope.nodeId))
			throw new HttpError(404, 'not_found', 'That controller was not found');
		const outs = scopeTargets(show, scope);
		if (!outs.length)
			throw new HttpError(
				400,
				'bad_request',
				'No props are wired to controller outputs here yet. Set up wiring first.'
			);
		const { plan, targets, schedule } = buildPlan(show, outs, {
			bitMs: body?.bitMs,
			level: body?.level,
			passes: body?.passes
		});
		const run: StoredRun = { id: newId(), kind: 'map', startedAt: nowIso(), scope, plan, targets };
		runs.unshift(run);
		ctx.log('info', `Camera mapping started on ${targets.length} outputs`);
		return startResponse(run, schedule);
	});
	ctx.route('POST', '/mapping/frame', () => ({ ok: true }));
	ctx.route('GET', '/mapping/runs', () =>
		runs.map((r) => ({ ...r, results: r.results && { ...r.results, detected: [] } }))
	);
	ctx.route('GET', '/mapping/runs/([^/]+)', ({ params }) => find(params[0]));
	ctx.route('DELETE', '/mapping/runs/([^/]+)', ({ params }) => {
		find(params[0]);
		runs.splice(
			runs.findIndex((r) => r.id === params[0]),
			1
		);
		return { ok: true };
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/stop', ({ params }) => {
		find(params[0]);
		return { ok: true, stopped: true };
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/results', ({ params, body }) => {
		const r = find(params[0]);
		r.results = { detected: body?.detected ?? [], proposals: body?.proposals ?? [], stats: body?.stats };
		return r;
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/photo/background', ({ params }) => {
		find(params[0]);
		return { ok: true, path: 'layout/background.jpg' };
	});
	ctx.route('POST', '/mapping/runs/([^/]+)/apply', ({ params, body }) => {
		const r = find(params[0]);
		const ids: string[] = body?.proposalIds ?? [];
		if (!ids.length) throw new HttpError(400, 'bad_request', 'Pick at least one change to apply.');
		const chosen = ids.map((id) => {
			const p = r.results?.proposals.find((x) => x.id === id);
			if (!p) throw new HttpError(404, 'not_found', 'A selected proposal was not found');
			return p;
		});
		const snapshotId = `demo-${Date.now()}`;
		ctx.server.snapshots.unshift({
			id: snapshotId,
			label: 'Before camera mapping',
			createdAt: nowIso(),
			sizeBytes: 1024,
			auto: true,
			showVersion: ctx.server.show.version
		});
		const applied = applyProposals(ctx.server.show, chosen);
		r.appliedSnapshotId = snapshotId;
		r.appliedProposalIds = [...new Set([...(r.appliedProposalIds ?? []), ...ids])];
		ctx.bump();
		return { show: ctx.server.show, snapshotId, applied };
	});
}

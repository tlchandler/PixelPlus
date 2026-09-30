// WS4 (F7): pixel-count check, mirroring the daemon (api/pixelcount.rs). The
// manual search runs against a demo string that is 2 pixels shorter than
// configured; the camera method returns a phase-B plan like the daemon.
import { PHASE_B } from '$lib/cv/mapcode';
import { buildPlan, outputLength, probeLen } from '$lib/cv/plan';
import type { CountStep, StoredRun } from '$lib/cv/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';
import { runs, startResponse } from './mapping';

interface Session {
	nodeId: string;
	output: number;
	configured: number;
	maxProbe: number;
	lo: number;
	hi: number;
	history: [number, number][];
	/** The demo string's real length. */
	actual: number;
}

export function register(ctx: FeatureContext) {
	const sessions = new Map<string, Session>();
	const step = (id: string): CountStep => {
		const s = sessions.get(id)!;
		const base = {
			session: id,
			nodeId: s.nodeId,
			output: s.output,
			configured: s.configured,
			maxProbe: s.maxProbe,
			canUndo: s.history.length > 0
		};
		if (s.lo >= s.hi) return { ...base, count: s.lo };
		const k = s.lo + Math.floor((s.hi - s.lo) / 2);
		const remaining = Math.ceil(Math.log2(s.hi - s.lo + 1));
		return {
			...base,
			step: {
				litUntil: k,
				number: s.history.length + 1,
				maxRemaining: remaining,
				ask:
					k === 0
						? 'Only the first pixel should be lit, in red. Can you see it?'
						: `Pixels 1–${k} are dim green and pixel ${k + 1} should be red. Can you see a red pixel at the end of the green run?`
			}
		};
	};
	const get = (id: string) => {
		const s = sessions.get(id);
		if (!s) throw new HttpError(409, 'no_session', 'That pixel-count check has ended. Start it again.');
		return s;
	};
	ctx.route('POST', '/pixelcount/start', ({ body }) => {
		const show = ctx.server.show;
		const node = show.nodes.find((n) => n.id === body?.nodeId);
		if (!node) throw new HttpError(404, 'not_found', 'That controller was not found');
		if (!body?.output || !node.outputs.some((o) => o.index === body.output))
			throw new HttpError(400, 'bad_request', `${node.name} doesn't have output ${body?.output}.`);
		const configured = outputLength(show, node.id, body.output);
		const maxProbe = probeLen(Math.max(1, configured));
		if (body.method === 'camera') {
			const { plan, targets, schedule } = buildPlan(
				show,
				[[node.id, body.output]],
				{ probeExtra: true, bitMs: body.bitMs },
				PHASE_B
			);
			const run: StoredRun = {
				id: newId(),
				kind: 'pixelCount',
				startedAt: nowIso(),
				scope: { nodeId: node.id },
				plan,
				targets
			};
			runs.unshift(run);
			return {
				...startResponse(run, schedule),
				configured,
				maxProbe: plan.targets[0].maxPixels,
				limited: false
			};
		}
		if (body.method === 'current')
			throw new HttpError(
				400,
				'bad_request',
				"Measuring by current needs a current sensor on the receiver's power feed (an ESP32 sensor node with an INA226). Use the camera or the manual check."
			);
		const id = newId();
		sessions.set(id, {
			nodeId: node.id,
			output: body.output,
			configured,
			maxProbe,
			lo: 0,
			hi: maxProbe,
			history: [],
			actual: Math.max(0, configured - 2)
		});
		return { ...step(id), limited: false };
	});
	ctx.route('POST', '/pixelcount/([^/]+)/answer', ({ params, body }) => {
		const s = get(params[0]);
		if (s.lo < s.hi) {
			const k = s.lo + Math.floor((s.hi - s.lo) / 2);
			s.history.push([s.lo, s.hi]);
			if (body?.seen) s.lo = k + 1;
			else s.hi = k;
		}
		return step(params[0]);
	});
	ctx.route('POST', '/pixelcount/([^/]+)/undo', ({ params }) => {
		const s = get(params[0]);
		const h = s.history.pop();
		if (h) [s.lo, s.hi] = h;
		return step(params[0]);
	});
	ctx.route('POST', '/pixelcount/([^/]+)/stop', ({ params }) => {
		sessions.delete(params[0]);
		return { ok: true };
	});
	ctx.route('POST', '/pixelcount/([^/]+)/result', ({ params, body }) => {
		const r = runs.find((x) => x.id === params[0]);
		if (!r) throw new HttpError(404, 'not_found', 'That mapping run was not found');
		r.results = { detected: [], proposals: [], stats: { count: body?.count, dead: body?.dead ?? [] } };
		return { ok: true, count: body?.count, configured: r.targets[0]?.configured ?? 0 };
	});
	ctx.route('POST', '/pixelcount/([^/]+)/apply', ({ params, body }) => {
		const show = ctx.server.show;
		const s = sessions.get(params[0]);
		const r = runs.find((x) => x.id === params[0]);
		const nodeId = s?.nodeId ?? r?.targets[0]?.nodeId;
		const output = s?.output ?? r?.targets[0]?.output;
		const count = body?.count ?? (s ? s.lo : (r?.results?.stats?.count as number | undefined));
		if (!nodeId || !output || count == null)
			throw new HttpError(409, 'no_session', 'That pixel-count check has ended.');
		const out = show.nodes.find((n) => n.id === nodeId)?.outputs.find((o) => o.index === output);
		if (out)
			out.measuredPixels = { count, method: s ? 'manual' : 'camera', at: nowIso(), dead: body?.dead ?? [] };
		let message = `${count} pixels measured`;
		if (body?.updatePropCount) {
			const segs = show.props
				.flatMap((p) => p.segments.map((sg) => ({ p, sg })))
				.filter((x) => x.sg.nodeId === nodeId && x.sg.output === output);
			const last = segs.sort((a, b) => b.sg.startPixel - a.sg.startPixel)[0];
			if (!last) throw new HttpError(400, 'bad_request', 'No prop is wired to that output.');
			const n = count - last.sg.startPixel;
			if (n <= 0) throw new HttpError(400, 'bad_request', 'Check the wiring before changing counts.');
			last.p.pixelCount += n - last.sg.pixelCount;
			last.sg.pixelCount = n;
			message = `"${last.p.name}" now has ${last.p.pixelCount} pixels`;
		}
		sessions.delete(params[0]);
		ctx.bump();
		return { show, snapshotId: `demo-${Date.now()}`, message };
	});
}

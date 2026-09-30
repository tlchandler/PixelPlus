// WS4 (F7): pixel-count check (manual binary search simulated against a string of 48).
import type { PixelCountStep } from '$lib/api/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError } from './context';

export function register(ctx: FeatureContext) {
	const sessions = new Map<string, { lo: number; hi: number; actual: number }>();
	const step = (id: string): PixelCountStep => {
		const s = sessions.get(id)!;
		if (s.hi - s.lo <= 1) return { session: id, count: s.lo + 1 };
		const k = Math.floor((s.lo + s.hi) / 2);
		return { session: id, step: { litUntil: k, ask: `Is pixel ${k + 1} lit (red)?` } };
	};
	ctx.route('POST', '/pixelcount/start', ({ body }) => {
		if (!body?.nodeId || !body?.output) throw new HttpError(400, 'bad_request', 'Pick an output.');
		const id = newId();
		if (body.method === 'camera')
			return {
				runId: id,
				plan: {
					seed: 1,
					bitMs: 200,
					level: 77,
					passes: 3,
					phases: 2,
					targets: [{ nodeId: body.nodeId, output: body.output, maxPixels: 128 }],
					pixelBits: 7,
					startPosMs: 0
				}
			};
		sessions.set(id, { lo: 0, hi: 128, actual: 48 });
		return step(id);
	});
	ctx.route('POST', '/pixelcount/([^/]+)/answer', ({ params, body }) => {
		const s = sessions.get(params[0]);
		if (!s) throw new HttpError(404, 'not_found', 'That check has ended.');
		const k = Math.floor((s.lo + s.hi) / 2);
		if (body?.seen) s.lo = k;
		else s.hi = k;
		return step(params[0]);
	});
	ctx.route('POST', '/pixelcount/([^/]+)/result', ({ body }) => ({ ok: true, count: body?.count }));
	ctx.route('POST', '/pixelcount/([^/]+)/apply', () => {
		ctx.bump();
		return { ok: true };
	});
}

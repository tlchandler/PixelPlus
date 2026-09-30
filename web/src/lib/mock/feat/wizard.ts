// WS4 (F9): "Add receiver" wizard.
import type { JackCandidate } from '$lib/api/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError } from './context';

const COLORS = ['#ffffff', '#ff0000', '#00ff00', '#0000ff'];

export function register(ctx: FeatureContext) {
	const sessions = new Map<string, { nodeId: string; jack?: number }>();
	const get = (id: string) => {
		const s = sessions.get(id);
		if (!s) throw new HttpError(404, 'not_found', 'That wizard has ended.');
		return s;
	};
	ctx.route('POST', '/wizard/receiver/identify-jack', ({ body }) => {
		const show = ctx.server.show;
		const node = show.nodes.find((n) => n.id === body?.nodeId);
		if (!node) throw new HttpError(404, 'not_found', 'That controller');
		const used = new Set(show.receivers.filter((r) => r.nodeId === node.id).map((r) => r.jack));
		const jacks = Math.floor(node.outputs.length / 4);
		const candidates: JackCandidate[] = Array.from({ length: jacks }, (_, i) => i + 1)
			.filter((j) => !used.has(j))
			.slice(0, 16)
			.map((jack, i) => ({ jack, color: COLORS[i % 4], blinks: 1 + Math.floor(i / 4) }));
		const sessionId = newId();
		sessions.set(sessionId, { nodeId: node.id });
		return { sessionId, candidates };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/jack', ({ params, body }) => {
		get(params[0]).jack = body?.jack;
		return { ok: true };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/port/(\\d+)/light', ({ params }) => {
		get(params[0]);
		return { ok: true, port: Number(params[1]) };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/finish', ({ params, body }) => {
		const s = get(params[0]);
		const show = ctx.server.show;
		show.receivers.push({
			id: newId(),
			name: body?.receiver?.name ?? `Receiver J${s.jack ?? 1}`,
			kind: 'diffrx',
			nodeId: s.nodeId,
			jack: s.jack ?? 1,
			fuseAmps: 6
		});
		sessions.delete(params[0]);
		ctx.bump();
		return show;
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/cancel', ({ params }) => {
		sessions.delete(params[0]);
		return { ok: true };
	});
}

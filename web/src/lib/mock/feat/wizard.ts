// WS4 (F9): "Add receiver" wizard, mirroring the daemon (api/wizard.rs). The
// demo receiver is plugged into the highest free jack.
import type { ColorOrder, PropSegment } from '$lib/api/types';
import { BOARDS } from '$lib/util/boards';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError } from './context';

const SIGNALS: [string, number][] = [
	['#707070', 1],
	['#707070', 2],
	['#707070', 3],
	['#707070', 4],
	['#0000c0', 1],
	['#0000c0', 2],
	['#0000c0', 3],
	['#0000c0', 4]
];
const ORDERS: ColorOrder[] = ['RGB', 'RBG', 'GRB', 'GBR', 'BRG', 'BGR'];
const SRC: Record<ColorOrder, number[]> = {
	RGB: [0, 1, 2],
	RBG: [0, 2, 1],
	GRB: [1, 0, 2],
	GBR: [1, 2, 0],
	BRG: [2, 0, 1],
	BGR: [2, 1, 0]
};

/** The strip's real colour order from what pure red and green looked like (0 = red, 1 = green, 2 = blue). */
export function detectColorOrder(
	configured: ColorOrder,
	seenRed: number,
	seenGreen: number
): ColorOrder | null {
	if (seenRed === seenGreen || seenRed > 2 || seenGreen > 2) return null;
	const src = SRC[configured];
	const truth = [0, 0, 0];
	truth[src.indexOf(0)] = seenRed;
	truth[src.indexOf(1)] = seenGreen;
	truth[src.indexOf(2)] = 3 - seenRed - seenGreen;
	return ORDERS.find((o) => SRC[o].every((v, i) => v === truth[i])) ?? null;
}

export function register(ctx: FeatureContext) {
	const sessions = new Map<string, { nodeId: string; candidates: number[]; jack?: number }>();
	const get = (id: string) => {
		const s = sessions.get(id);
		if (!s) throw new HttpError(409, 'no_session', 'This wizard has ended. Start "Add receiver" again.');
		return s;
	};
	const signals = (c: number[]) =>
		c.map((jack, i) => ({ jack, color: SIGNALS[i % 8][0], blinks: SIGNALS[i % 8][1] }));
	ctx.route('POST', '/wizard/receiver/identify-jack', ({ body }) => {
		const show = ctx.server.show;
		const node = show.nodes.find((n) => n.id === body?.nodeId);
		if (!node) throw new HttpError(404, 'not_found', 'That controller was not found');
		const jacks = Math.max(BOARDS[node.board]?.jacks ?? 0, Math.floor(node.outputs.length / 4));
		if (!jacks)
			throw new HttpError(
				400,
				'bad_request',
				`${node.name} has no receiver jacks; wire props to its outputs directly.`
			);
		const used = new Set(show.receivers.filter((r) => r.nodeId === node.id).map((r) => r.jack));
		const free = Array.from({ length: jacks }, (_, i) => i + 1).filter((j) => !used.has(j));
		if (!free.length)
			throw new HttpError(400, 'bad_request', `Every jack on ${node.name} already has a receiver.`);
		const sessionId = newId();
		sessions.set(sessionId, { nodeId: node.id, candidates: free });
		return {
			sessionId,
			method: 'identify',
			round: free.length > 8 ? 'first' : 'final',
			candidates: signals(free),
			probeJack: null
		};
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/pick', ({ params, body }) => {
		const s = get(params[0]);
		const left = signals(s.candidates)
			.filter((c) => c.color.toLowerCase() === String(body?.color).toLowerCase() && c.blinks === body?.blinks)
			.map((c) => c.jack);
		if (!left.length) throw new HttpError(400, 'bad_request', 'None of the jacks shows that signal.');
		s.candidates = left;
		if (left.length === 1) {
			s.jack = left[0];
			return { done: true, jack: left[0] };
		}
		return {
			done: false,
			sessionId: params[0],
			method: 'identify',
			round: 'final',
			candidates: signals(left)
		};
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/probe', ({ params, body }) => {
		get(params[0]);
		return { ok: true, jack: body?.jack };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/jack', ({ params, body }) => {
		get(params[0]).jack = body?.jack;
		return { ok: true, jack: body?.jack };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/port/(\\d+)/light', ({ params }) => {
		const s = get(params[0]);
		if (!s.jack) throw new HttpError(400, 'bad_request', 'Find the jack first.');
		return { ok: true, output: (s.jack - 1) * 4 + Number(params[1]) };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/color-order', ({ params, body }) => {
		const s = get(params[0]);
		const idx = (c: string) => ['red', 'green', 'blue'].indexOf(c);
		const out = ctx.server.show.nodes
			.find((n) => n.id === s.nodeId)
			?.outputs.find((o) => o.index === ((s.jack ?? 1) - 1) * 4 + body?.port);
		const configured = out?.colorOrder ?? 'RGB';
		const order = detectColorOrder(configured, idx(body?.red), idx(body?.green));
		if (!order) throw new HttpError(400, 'bad_request', "Red and green can't look the same; try again.");
		return { colorOrder: order, configured, changed: order !== configured };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/finish', ({ params, body }) => {
		const s = get(params[0]);
		if (!s.jack) throw new HttpError(400, 'bad_request', 'Find the jack first.');
		const show = ctx.server.show;
		const name = String(body?.receiver?.name ?? '').trim();
		if (!name) throw new HttpError(400, 'bad_request', 'Give the receiver a name.');
		const kind = body?.receiver?.kind ?? 'diffrx';
		const receiver = {
			id: newId(),
			name,
			kind,
			nodeId: s.nodeId,
			jack: s.jack,
			location: body?.receiver?.location || undefined,
			fuseAmps: body?.receiver?.fuseAmps ?? (kind === 'diffrx' || kind === 'diffsmart-rx' ? 6 : undefined)
		};
		show.receivers.push(receiver);
		const node = show.nodes.find((n) => n.id === s.nodeId);
		for (const port of body?.ports ?? []) {
			const output = (s.jack - 1) * 4 + port.port;
			const out = node?.outputs.find((o) => o.index === output);
			if (out && port.colorOrder) out.colorOrder = port.colorOrder;
			let start = 0;
			for (const pid of port.propIds ?? []) {
				const prop = show.props.find((p) => p.id === pid);
				if (!prop) continue;
				const seg: PropSegment = {
					nodeId: s.nodeId,
					output,
					startPixel: start,
					pixelCount: prop.pixelCount,
					propOffset: 0,
					reverse: !!port.reverse,
					nullPixels: 0
				};
				prop.segments = [seg];
				start += prop.pixelCount;
			}
		}
		sessions.delete(params[0]);
		ctx.bump();
		return { show, receiver, snapshotId: `demo-${Date.now()}` };
	});
	ctx.route('POST', '/wizard/receiver/([^/]+)/cancel', ({ params }) => {
		sessions.delete(params[0]);
		return { ok: true };
	});
}

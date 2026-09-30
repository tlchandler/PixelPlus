// WS6 (F20): ESP32 sensor nodes and surprises (mirrors api/sensornodes.rs).
import type { DiscoveredSensorNode, SensorNode } from '$lib/api/types';
import type { SensorLive } from '$lib/insight/api';
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';

export function register(ctx: FeatureContext) {
	const list = () => (ctx.server.show.sensorNodes ??= []);
	const find = (id: string) => {
		const n = list().find((x) => x.id === id);
		if (!n) throw new HttpError(404, 'not_found', 'That sensor was not found');
		return n;
	};
	const discovered: DiscoveredSensorNode[] = [
		{
			id: 'sn9c1e2a00',
			name: 'PixelPlus-Sensor-2A00',
			hw: 'esp32c3',
			ver: '0.1.0',
			ip: '192.168.1.81',
			adoptedBy: null,
			inputs: ['pir1', 'btn1']
		}
	];
	const live: Record<string, SensorLive> = {};
	const liveOf = (n: SensorNode): SensorLive =>
		(live[n.id] ??= {
			online: true,
			rssi: -61,
			uptimeS: 86400,
			ip: '192.168.1.81',
			ver: '0.1.0',
			lastSeen: nowIso(),
			inputs: Object.fromEntries(n.inputs.filter((i) => i.kind !== 'current').map((i) => [i.id, 0])),
			amps: Object.fromEntries(n.inputs.filter((i) => i.kind === 'current').map((i) => [i.id, 3.2])),
			volts: {},
			events: 0,
			rejected: 0
		});

	// Demo: the first motion input trips now and then (browser demo only).
	if (typeof window !== 'undefined')
		setInterval(() => {
			const n = list()[0];
			const inp = n?.inputs.find((i) => i.kind === 'motion');
			if (!n || !inp) return;
			const l = liveOf(n);
			for (const v of [1, 0]) {
				setTimeout(
					() => {
						l.inputs[inp.id] = v;
						if (v) l.events++;
						ctx.broadcast('sensorInput', { sensorNodeId: n.id, input: inp.id, state: v, at: nowIso() });
					},
					v ? 0 : 4000
				);
			}
		}, 25000);

	ctx.route('GET', '/sensor-nodes/discovered', () =>
		discovered.filter((d) => !list().some((n) => n.id === d.id && n.adopted))
	);
	ctx.route('GET', '/sensor-nodes/live', () => Object.fromEntries(list().map((n) => [n.id, liveOf(n)])));
	ctx.route('POST', '/sensor-nodes/adopt', ({ body }) => {
		const d = discovered.find((x) => x.id === body?.id);
		if (!d) throw new HttpError(404, 'not_found', 'That sensor was not found');
		d.adoptedBy = 'nmain00001';
		const node: SensorNode = {
			id: d.id,
			name: d.name,
			hw: d.hw,
			adopted: true,
			inputs: d.inputs.map((id, i) => ({
				id,
				name: id.startsWith('pir') ? `Motion ${i + 1}` : `Button ${i + 1}`,
				pin: 4 + i,
				kind: id.startsWith('pir') ? ('motion' as const) : ('button' as const),
				activeLow: !id.startsWith('pir'),
				debounceMs: 30,
				holdMs: id.startsWith('pir') ? 5000 : 0
			}))
		};
		list().push(node);
		ctx.bump();
		return node;
	});
	ctx.route('POST', '/sensor-nodes/([^/]+)/release', ({ params }) => {
		const i = list().findIndex((x) => x.id === params[0]);
		if (i < 0) throw new HttpError(404, 'not_found', 'That sensor was not found');
		list().splice(i, 1);
		const d = discovered.find((x) => x.id === params[0]);
		if (d) d.adoptedBy = null;
		ctx.bump();
		return { ok: true };
	});
	ctx.route('POST', '/sensor-nodes/([^/]+)/identify', ({ params }) => {
		find(params[0]);
		return { ok: true };
	});
	ctx.route('GET', '/sensor-nodes/([^/]+)/live', ({ params }) => liveOf(find(params[0])));
	ctx.route('GET', '/sensor-nodes', () => list());
	ctx.route('GET', '/sensor-nodes/([^/]+)', ({ params }) => find(params[0]));
	ctx.route('PUT', '/sensor-nodes/([^/]+)', ({ params, body }) => {
		const n = find(params[0]);
		if (body?.name !== undefined) n.name = String(body.name).trim() || n.name;
		if (body?.location !== undefined) n.location = body.location || undefined;
		if (Array.isArray(body?.inputs)) n.inputs = body.inputs;
		ctx.bump();
		return n;
	});
	ctx.route('DELETE', '/sensor-nodes/([^/]+)', ({ params }) => {
		const i = list().findIndex((x) => x.id === params[0]);
		if (i >= 0) list().splice(i, 1);
		ctx.bump();
		return { ok: true };
	});
	ctx.route('POST', '/surprises/test', ({ body }) => {
		const ref = body?.action?.ref;
		const name =
			ctx.server.show.effects.find((e) => e.id === ref)?.name ??
			ctx.server.show.sequences.find((s) => s.id === ref)?.name ??
			'Surprise';
		return { ok: true, message: `${name} played over the song` };
	});
	ctx.route('POST', '/triggers/([^/]+)/fire', ({ params }) => {
		const t = ctx.server.show.settings.triggers.find((x) => x.id === params[0]);
		if (!t) throw new HttpError(404, 'not_found', 'That trigger was not found');
		return { ok: true, message: `Ran "${t.name}"` };
	});
}

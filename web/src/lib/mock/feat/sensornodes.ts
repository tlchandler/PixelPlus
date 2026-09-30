// WS6 (F20): ESP32 sensor nodes and surprises.
import type { DiscoveredSensorNode } from '$lib/api/types';
import type { FeatureContext } from './context';
import { crud, HttpError } from './context';

export function register(ctx: FeatureContext) {
	const list = () => (ctx.server.show.sensorNodes ??= []);
	const discovered: DiscoveredSensorNode[] = [
		{
			id: 'sn9c1e2a00',
			name: 'PixelPlus-Sensor-9C1E',
			hw: 'esp32c3',
			ver: '0.1.0',
			ip: '192.168.1.81',
			adoptedBy: null,
			inputs: ['pir1']
		}
	];
	ctx.route('GET', '/sensor-nodes/discovered', () => discovered);
	ctx.route('POST', '/sensor-nodes/adopt', ({ body }) => {
		const d = discovered.find((x) => x.id === body?.id);
		if (!d) throw new HttpError(404, 'not_found', 'That sensor');
		d.adoptedBy = 'nmain00001';
		const node = {
			id: d.id,
			name: d.name,
			hw: d.hw,
			adopted: true,
			inputs: d.inputs.map((id, i) => ({
				id,
				name: `Input ${i + 1}`,
				pin: 4 + i,
				kind: 'motion' as const,
				activeLow: false,
				debounceMs: 30,
				holdMs: 0
			}))
		};
		list().push(node);
		ctx.bump();
		return node;
	});
	ctx.route('POST', '/sensor-nodes/([^/]+)/release', ({ params }) => {
		const i = list().findIndex((x) => x.id === params[0]);
		if (i >= 0) list().splice(i, 1);
		ctx.bump();
		return { ok: true };
	});
	ctx.route('GET', '/sensor-nodes/([^/]+)/live', ({ params }) => {
		const n = list().find((x) => x.id === params[0]);
		if (!n) throw new HttpError(404, 'not_found', 'That sensor');
		return { rssi: -61, uptimeS: 86400, inputs: Object.fromEntries(n.inputs.map((i) => [i.id, 0])) };
	});
	crud(ctx, '/sensor-nodes', list);
	ctx.route('POST', '/surprises/test', () => ({ ok: true, message: 'Surprise played' }));
}

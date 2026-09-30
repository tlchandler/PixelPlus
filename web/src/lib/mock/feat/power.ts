// WS3 (F12): power supplies and live power.
import type { PowerLive } from '$lib/api/types';
import type { FeatureContext } from './context';
import { crud } from './context';

export function register(ctx: FeatureContext) {
	crud(ctx, '/power-supplies', () => (ctx.server.show.powerSupplies ??= []));
	ctx.route('GET', '/power/live', (): PowerLive => {
		const playing = ctx.server.play.state === 'playing';
		return {
			nodes: ctx.server.show.nodes.map((n) => ({
				nodeId: n.id,
				groups: [{ id: `${n.id}-bus`, amps: playing ? 8 + Math.random() * 6 : 0.4, budget: 27, scale: 1 }]
			}))
		};
	});
}

// WS5 (F15): cluster updates (the base `GET/POST /system/update` stay in server.ts).
import type { FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	ctx.route('PUT', '/system/update/settings', ({ body }) => {
		Object.assign(
			(ctx.server.show.settings.updates ??= {
				channel: 'stable',
				auto: 'notify',
				window: { from: '10:00', to: '14:00', days: [] },
				avoidShowHours: 2
			}),
			body ?? {}
		);
		ctx.bump();
		return ctx.server.show.settings.updates;
	});
	ctx.route('POST', '/system/update/rollback', () =>
		ctx.server.runHelper('update-rollback', 'Rolling back…', 'Rolled back to the previous version')
	);
}

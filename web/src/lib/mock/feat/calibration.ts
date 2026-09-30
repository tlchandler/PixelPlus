// WS1 (F1): POST /calibration/result. (`POST /player/calibration {pattern:'v2'}` is answered
// by server.ts with a demo pattern.)
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';

export function register(ctx: FeatureContext) {
	ctx.route('POST', '/calibration/result', ({ body }) => {
		const audio = ctx.server.show.settings.audio;
		if (typeof body?.residualMs !== 'number')
			throw new HttpError(400, 'bad_request', 'residualMs is missing.');
		const current = audio.outputDelayMs ?? 0;
		const next = Math.max(-500, Math.min(2000, current + Math.round(body.residualMs)));
		const calibration = {
			measuredAt: nowIso(),
			method: 'phone' as const,
			residualMs: body.residualMs,
			spreadMs: body.spreadMs ?? 0,
			matches: body.matches ?? 0,
			appliedDelayMs: body.apply ? next : current,
			device: body.device
		};
		if (body.apply) {
			audio.outputDelayMs = next;
			audio.lastCalibration = calibration;
			ctx.bump();
		}
		return { outputDelayMs: audio.outputDelayMs ?? 0, calibration };
	});
}

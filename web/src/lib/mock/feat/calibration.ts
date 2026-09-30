// WS1 (F1): POST /calibration/result and /calibration/undo. (`POST /player/calibration
// {pattern:'v2'}` is answered by server.ts with a demo pattern.)
import type { FeatureContext } from './context';
import { HttpError, nowIso } from './context';

export function register(ctx: FeatureContext) {
	ctx.route('POST', '/calibration/result', ({ body }) => {
		const audio = ctx.server.show.settings.audio;
		if (
			typeof body?.residualMs !== 'number' ||
			!Number.isFinite(body.residualMs) ||
			Math.abs(body.residualMs) > 3000
		)
			throw new HttpError(400, 'bad_request', 'That measurement is out of range; please measure again.');
		const current = audio.outputDelayMs ?? 0;
		const want = current + Math.round(body.residualMs);
		const next = Math.max(-500, Math.min(2000, want));
		const previousCalibration = audio.lastCalibration ?? null;
		const calibration = {
			measuredAt: nowIso(),
			method: 'phone' as const,
			residualMs: Math.round(body.residualMs * 10) / 10,
			spreadMs: Math.round((body.spreadMs ?? 0) * 10) / 10,
			matches: body.matches ?? 0,
			appliedDelayMs: body.apply ? next : current,
			...(body.device ? { device: String(body.device).slice(0, 80) } : {})
		};
		if (body.apply) {
			audio.outputDelayMs = next;
			audio.lastCalibration = calibration;
			ctx.bump();
		}
		return {
			outputDelayMs: audio.outputDelayMs ?? 0,
			previousDelayMs: current,
			previousCalibration,
			calibration,
			clamped: next !== want
		};
	});
	ctx.route('POST', '/calibration/undo', ({ body }) => {
		const audio = ctx.server.show.settings.audio;
		const ms = body?.outputDelayMs;
		if (typeof ms !== 'number' || ms < -500 || ms > 2000)
			throw new HttpError(400, 'bad_request', 'The sound delay must be between -500 and 2000 ms.');
		audio.outputDelayMs = ms;
		if (body.lastCalibration) audio.lastCalibration = body.lastCalibration;
		else delete audio.lastCalibration;
		ctx.bump();
		return { outputDelayMs: ms };
	});
}

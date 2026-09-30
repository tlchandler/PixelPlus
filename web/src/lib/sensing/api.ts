/** Endpoints for the secure connection (F1) and phone calibration. */
import { request } from '$lib/api/client';
import type { AudioCalibration, TlsStatus } from '$lib/api/types';
import { planFromReply, type CalPlan } from './schedule';

/** `GET /public/tls` (no sign-in): what the trust page shows. */
export interface PublicTls {
	available: boolean;
	role: string;
	port: number;
	caFingerprint: string | null;
	caSubject: string | null;
	leafNames: string[];
	urls: string[];
	secureNow: boolean;
}

export interface CalibrationApplied {
	outputDelayMs: number;
	previousDelayMs: number;
	previousCalibration?: AudioCalibration | null;
	calibration: AudioCalibration;
	clamped: boolean;
}

export interface StartedPattern {
	plan: CalPlan;
	/** Phone time (performance.now()) of pattern position 0, when the leader said. */
	pos0Ms?: number;
}

export const tlsApi = {
	status: () => request<TlsStatus>('GET', '/tls/status'),
	publicStatus: () => request<PublicTls>('GET', '/public/tls'),
	rotate: (ca: boolean) => request<TlsStatus>('POST', '/tls/rotate', { ca }),
	caUrl: '/api/v1/public/ca.crt',
	mobileconfigUrl: '/api/v1/public/ca.mobileconfig'
};

export const calibrationApi = {
	/**
	 * (Re)start the v2 flash/click pattern. Each call restarts it with a new seed, from
	 * position 0; the reply time brackets when that happens.
	 */
	async start(): Promise<StartedPattern> {
		const t0 = performance.now();
		const reply = await request<Record<string, unknown>>('POST', '/player/calibration', {
			on: true,
			pattern: 'v2'
		});
		const t1 = performance.now();
		const plan = planFromReply(reply);
		const pos0Ms = plan.startsInMs !== undefined ? (t0 + t1) / 2 + plan.startsInMs : undefined;
		return { plan, pos0Ms };
	},
	stop: () => request('POST', '/player/calibration', { on: false }),
	/** Stop even while the page is closing. */
	stopBeacon(): void {
		try {
			void fetch('/api/v1/player/calibration', {
				method: 'POST',
				keepalive: true,
				credentials: 'same-origin',
				headers: { 'content-type': 'application/json', 'x-pixelplus-request': '1' },
				body: JSON.stringify({ on: false })
			}).catch(() => {});
		} catch {
			/* page is going away */
		}
	},
	result: (b: { residualMs: number; spreadMs: number; matches: number; device?: string; apply: boolean }) =>
		request<CalibrationApplied>('POST', '/calibration/result', b),
	undo: (outputDelayMs: number, lastCalibration: AudioCalibration | null | undefined) =>
		request<{ outputDelayMs: number }>('POST', '/calibration/undo', {
			outputDelayMs,
			lastCalibration: lastCalibration ?? null
		})
};

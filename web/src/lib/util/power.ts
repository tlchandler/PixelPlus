import type { Prop } from '$lib/api/types';

/** Full-white worst case and a typical show average (≈ 1/3 of peak) in amps. */
export function propPower(p: Prop): { peak: number; typical: number } {
	const peak = (p.pixelCount * (p.maxMilliampsPerPixel ?? 60)) / 1000;
	return { peak, typical: peak * 0.32 };
}

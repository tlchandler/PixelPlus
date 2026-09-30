// NOAA sunrise/sunset approximation (accurate to ~1 minute), no dependencies.

const rad = Math.PI / 180;

function dayOfYear(d: Date): number {
	const start = Date.UTC(d.getUTCFullYear(), 0, 0);
	return Math.floor((d.getTime() - start) / 86400000);
}

/**
 * Returns the UTC instant of sunset (or sunrise) on the given calendar date
 * (year/month/day interpreted as the local date at that longitude), or null in polar day/night.
 */
export function sunTime(
	year: number,
	month: number,
	day: number,
	lat: number,
	lon: number,
	kind: 'sunset' | 'sunrise'
): Date | null {
	const date = new Date(Date.UTC(year, month - 1, day, 12));
	const n = dayOfYear(date);
	const gamma = ((2 * Math.PI) / 365) * (n - 1);
	const eqtime =
		229.18 *
		(0.000075 +
			0.001868 * Math.cos(gamma) -
			0.032077 * Math.sin(gamma) -
			0.014615 * Math.cos(2 * gamma) -
			0.040849 * Math.sin(2 * gamma));
	const decl =
		0.006918 -
		0.399912 * Math.cos(gamma) +
		0.070257 * Math.sin(gamma) -
		0.006758 * Math.cos(2 * gamma) +
		0.000907 * Math.sin(2 * gamma) -
		0.002697 * Math.cos(3 * gamma) +
		0.00148 * Math.sin(3 * gamma);
	const zenith = 90.833 * rad;
	const cosH = Math.cos(zenith) / (Math.cos(lat * rad) * Math.cos(decl)) - Math.tan(lat * rad) * Math.tan(decl);
	if (cosH < -1 || cosH > 1) return null;
	const ha = Math.acos(cosH) / rad;
	const minutes = kind === 'sunrise' ? 720 - 4 * (lon + ha) - eqtime : 720 - 4 * (lon - ha) - eqtime;
	return new Date(Date.UTC(year, month - 1, day) + minutes * 60000);
}

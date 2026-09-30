import type { Show, TemperatureUnit } from '$lib/api/types';

/** IANA zones in the United States (the one big °F country). Canada and Mexico use °C. */
const US_ZONES =
	/^(America\/(New_York|Chicago|Denver|Los_Angeles|Phoenix|Anchorage|Juneau|Sitka|Yakutat|Nome|Metlakatla|Adak|Boise|Detroit|Menominee|Indiana\/.*|Kentucky\/.*|North_Dakota\/.*|Puerto_Rico)|Pacific\/Honolulu|US\/.*)$/;
/** Regions whose people read temperatures in °F. */
const F_REGIONS = new Set([
	'US',
	'PR',
	'GU',
	'VI',
	'AS',
	'MP',
	'UM',
	'LR',
	'BS',
	'BZ',
	'KY',
	'PW',
	'FM',
	'MH'
]);

/** °F for US visitors (by browser language region, else by time zone), °C everywhere else. */
export function localeTempUnit(language?: string, timeZone?: string): TemperatureUnit {
	let lang = language;
	let tz = timeZone;
	try {
		lang ??= typeof navigator !== 'undefined' ? navigator.language : undefined;
		tz ??= Intl.DateTimeFormat().resolvedOptions().timeZone;
	} catch {
		/* ignore */
	}
	const region = lang?.split(/[-_]/)[1]?.toUpperCase();
	if (region && region.length === 2) return F_REGIONS.has(region) ? 'f' : 'c';
	return tz && US_ZONES.test(tz) ? 'f' : 'c';
}

/** The unit this show displays temperatures in (setting, else the viewer's locale). */
export function tempUnitOf(show: Show | null | undefined): TemperatureUnit {
	return show?.settings.units?.temperature ?? localeTempUnit(undefined, show?.schedule.location.timezone);
}

export const cToF = (c: number) => (c * 9) / 5 + 32;
export const fToC = (f: number) => ((f - 32) * 5) / 9;

/** Celsius value converted to the display unit. */
export function tempValue(celsius: number, unit: TemperatureUnit): number {
	return unit === 'f' ? cToF(celsius) : celsius;
}

/** "72 °F" / "22 °C" from a Celsius reading. */
export function fmtTemp(celsius: number | null | undefined, unit: TemperatureUnit, digits = 0): string {
	if (celsius == null || !Number.isFinite(celsius)) return '—';
	return `${tempValue(celsius, unit).toFixed(digits)} °${unit === 'f' ? 'F' : 'C'}`;
}

export const tempSymbol = (unit: TemperatureUnit) => (unit === 'f' ? '°F' : '°C');

/** Wi-Fi signal strength (dBm) as 0–4 bars and a word. */
export function wifiQuality(dbm: number | null | undefined): { bars: number; label: string } {
	if (dbm == null || !Number.isFinite(dbm)) return { bars: 0, label: 'No signal' };
	if (dbm >= -55) return { bars: 4, label: 'Excellent' };
	if (dbm >= -67) return { bars: 3, label: 'Good' };
	if (dbm >= -75) return { bars: 2, label: 'Fair' };
	if (dbm >= -85) return { bars: 1, label: 'Weak' };
	return { bars: 0, label: 'Very weak' };
}

/** "United States" for "US" (falls back to the code). */
export function countryName(code: string): string {
	try {
		return new Intl.DisplayNames(['en'], { type: 'region' }).of(code) ?? code;
	} catch {
		return code;
	}
}

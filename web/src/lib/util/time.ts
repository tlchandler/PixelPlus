import type { Location, TimeSpec } from '$lib/api/types';
import { sunTime } from './sun';

/** Offset (ms) of `tz` from UTC at instant `d`. */
export function tzOffsetMs(d: Date, tz: string): number {
	try {
		const parts = new Intl.DateTimeFormat('en-US', {
			timeZone: tz,
			hourCycle: 'h23',
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit',
			second: '2-digit'
		}).formatToParts(d);
		const get = (t: string) => Number(parts.find((p) => p.type === t)?.value);
		const asUtc = Date.UTC(get('year'), get('month') - 1, get('day'), get('hour'), get('minute'), get('second'));
		return asUtc - Math.floor(d.getTime() / 1000) * 1000;
	} catch {
		return -d.getTimezoneOffset() * 60000;
	}
}

/** UTC instant for local wall-clock time in tz. */
export function zonedToUtc(y: number, m: number, d: number, hh: number, mm: number, tz: string): Date {
	const guess = Date.UTC(y, m - 1, d, hh, mm);
	const off = tzOffsetMs(new Date(guess), tz);
	const first = guess - off;
	const off2 = tzOffsetMs(new Date(first), tz);
	return new Date(guess - off2);
}

/** Calendar date (y, m, d) of an instant in tz. */
export function zonedParts(date: Date, tz: string): { y: number; m: number; d: number; dow: number } {
	const shifted = new Date(date.getTime() + tzOffsetMs(date, tz));
	return {
		y: shifted.getUTCFullYear(),
		m: shifted.getUTCMonth() + 1,
		d: shifted.getUTCDate(),
		dow: shifted.getUTCDay()
	};
}

/** Resolve a TimeSpec on a local calendar date → UTC instant. */
export function resolveTimeSpec(spec: TimeSpec, y: number, m: number, d: number, loc: Location): Date | null {
	if (spec.kind === 'clock') {
		const [hh, mm] = spec.time.split(':').map(Number);
		return zonedToUtc(y, m, d, hh || 0, mm || 0, loc.timezone);
	}
	const base = sunTime(y, m, d, loc.lat, loc.lon, spec.kind);
	if (!base) return null;
	return new Date(base.getTime() + spec.offsetMin * 60000);
}

export function fmtTime(d: Date | null | undefined, tz?: string): string {
	if (!d) return '—';
	return new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit', timeZone: tz }).format(d);
}

export function fmtDate(d: Date, tz?: string, opts: Intl.DateTimeFormatOptions = {}): string {
	return new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric', timeZone: tz, ...opts }).format(d);
}

export function describeTimeSpec(spec: TimeSpec): string {
	if (spec.kind === 'clock') {
		const [hh, mm] = spec.time.split(':').map(Number);
		const d = new Date(Date.UTC(2000, 0, 1, hh, mm));
		return new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit', timeZone: 'UTC' }).format(d);
	}
	const word = spec.kind === 'sunset' ? 'Sunset' : 'Sunrise';
	if (!spec.offsetMin) return word;
	const a = Math.abs(spec.offsetMin);
	const amt = a >= 60 && a % 60 === 0 ? `${a / 60} h` : a > 60 ? `${Math.floor(a / 60)} h ${a % 60} min` : `${a} min`;
	return `${amt} ${spec.offsetMin > 0 ? 'after' : 'before'} ${word.toLowerCase()}`;
}

/** "MM-DD" inside a (possibly year-wrapping) range. */
export function inDateRange(m: number, d: number, range?: { start: string; end: string }): boolean {
	if (!range) return true;
	const v = m * 100 + d;
	const [sm, sd] = range.start.split('-').map(Number);
	const [em, ed] = range.end.split('-').map(Number);
	const s = sm * 100 + sd;
	const e = em * 100 + ed;
	return s <= e ? v >= s && v <= e : v >= s || v <= e;
}

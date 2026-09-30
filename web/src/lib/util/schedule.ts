import type { Schedule, ScheduleEntry, ScheduleOccurrence, Weekday } from '$lib/api/types';
import { fmtDate, fmtTime, inDateRange, resolveTimeSpec, zonedParts } from './time';

const DOW: Weekday[] = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];

export interface Occurrence extends ScheduleOccurrence {
	priority: number;
	overridden?: boolean;
}

/** Expand schedule entries into concrete show windows (same rules as the daemon, §4 Schedule). */
export function expandSchedule(schedule: Schedule, from: Date, days: number): Occurrence[] {
	const loc = schedule.location;
	const out: Occurrence[] = [];
	const startParts = zonedParts(from, loc.timezone);
	for (let i = 0; i < days; i++) {
		const day = new Date(Date.UTC(startParts.y, startParts.m - 1, startParts.d + i, 12));
		const y = day.getUTCFullYear();
		const m = day.getUTCMonth() + 1;
		const d = day.getUTCDate();
		const dow = DOW[day.getUTCDay()];
		for (const e of schedule.entries) {
			if (!e.enabled || !e.days.includes(dow) || !inDateRange(m, d, e.dateRange)) continue;
			const s = resolveTimeSpec(e.start, y, m, d, loc);
			let en = resolveTimeSpec(e.end, y, m, d, loc);
			if (!s || !en) continue;
			if (en <= s) en = new Date(en.getTime() + 86400000);
			out.push({
				date: `${y}-${String(m).padStart(2, '0')}-${String(d).padStart(2, '0')}`,
				start: s.toISOString(),
				end: en.toISOString(),
				entryId: e.id,
				playlistId: e.playlistId,
				name: e.name,
				priority: e.priority
			});
		}
	}
	// Higher priority wins on overlap.
	for (const a of out) {
		for (const b of out) {
			if (a === b || a.overridden) continue;
			const overlap = a.start < b.end && b.start < a.end;
			if (overlap && b.priority > a.priority) a.overridden = true;
		}
	}
	return out.sort((a, b) => a.start.localeCompare(b.start));
}

export function nextShow(schedule: Schedule, now = new Date()): Occurrence | undefined {
	return expandSchedule(schedule, now, 30).find((o) => !o.overridden && new Date(o.end) > now);
}

/** The dashboard's line under "The show is resting": when the schedule starts the show next
 *  (nothing while the schedule is off), or that tonight's show was stopped while its window is on. */
export function restingLine(schedule: Schedule, now: Date): string {
	const next = schedule.enabled ? nextShow(schedule, now) : undefined;
	const tz = schedule.location.timezone;
	if (!next) return 'Nothing is scheduled. Press play to start any time.';
	const start = new Date(next.start);
	if (start <= now)
		return `Stopped during tonight's show (on until ${fmtTime(new Date(next.end), tz)}). Press play to bring it back.`;
	return `Starts automatically ${fmtDate(start, tz)} at ${fmtTime(start, tz)}.`;
}

export function entrySummary(e: ScheduleEntry): string {
	const all = e.days.length === 7;
	const wk =
		['mon', 'tue', 'wed', 'thu', 'fri'].every((d) => e.days.includes(d as Weekday)) && e.days.length === 5;
	const we = e.days.length === 2 && e.days.includes('sat') && e.days.includes('sun');
	if (all) return 'Every day';
	if (wk) return 'Weeknights';
	if (we) return 'Weekends';
	return e.days.map((d) => d[0].toUpperCase() + d.slice(1)).join(', ');
}

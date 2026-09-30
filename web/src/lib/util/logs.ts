export type LogLevel = 'error' | 'warn' | 'info' | 'debug';

export interface ParsedLog {
	time: Date | null;
	level: LogLevel;
	message: string;
	raw: string;
}

const ISO = /^(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)\s+(.*)$/;
const JOURNAL = /^\S+ [\w.@-]+(?:\[\d+\])?: (.*)$/;
const LEVEL = /^(ERROR|WARN(?:ING)?|INFO|DEBUG|TRACE)\b:?\s*(.*)$/i;
/** tracing's "pixelplus_daemon::player::engine: " target prefix */
const TARGET = /^[a-z_][\w]*(?:::[\w]+)+:\s+/;

function parseTime(s: string): Date | null {
	// journalctl short-iso writes "+0100"; Date wants "+01:00".
	const t = new Date(s.replace(' ', 'T').replace(/([+-]\d{2})(\d{2})$/, '$1:$2'));
	return Number.isNaN(t.getTime()) ? null : t;
}

/**
 * One line from `GET /system/logs`: the daemon's own "TIME  LEVEL  message" or a journalctl
 * "TIME host pixelplusd[pid]: …" line (which may carry the daemon's timestamp and level again).
 */
export function parseLogLine(raw: string): ParsedLog {
	let rest = raw.trim();
	let time: Date | null = null;
	let level: LogLevel = 'info';
	for (let pass = 0; pass < 2; pass++) {
		const m = ISO.exec(rest);
		if (m) {
			time = parseTime(m[1]) ?? time;
			rest = m[2];
		}
		const j = JOURNAL.exec(rest);
		if (j && pass === 0) rest = j[1];
	}
	const l = LEVEL.exec(rest);
	if (l) {
		const v = l[1].toLowerCase();
		level = v.startsWith('warn') ? 'warn' : v === 'error' ? 'error' : v === 'info' ? 'info' : 'debug';
		rest = l[2];
	}
	rest = rest.replace(TARGET, '');
	return { time, level, message: rest.trim(), raw };
}

/** All lines, newest first (lines without a time keep their relative order at the end). */
export function parseLogs(text: string): ParsedLog[] {
	const lines = text
		.split('\n')
		.filter((l) => l.trim())
		.map(parseLogLine);
	return lines
		.map((l, i) => ({ l, i }))
		.sort((a, b) => {
			const ta = a.l.time?.getTime();
			const tb = b.l.time?.getTime();
			if (ta != null && tb != null && ta !== tb) return tb - ta;
			if (ta == null && tb != null) return 1;
			if (tb == null && ta != null) return -1;
			return b.i - a.i;
		})
		.map((x) => x.l);
}

/** "Today", "Yesterday" or "Mon, Sep 28" for grouping log lines by day (local time). */
export function dayLabel(d: Date, now = new Date()): string {
	const key = (x: Date) => `${x.getFullYear()}-${x.getMonth()}-${x.getDate()}`;
	const y = new Date(now);
	y.setDate(y.getDate() - 1);
	if (key(d) === key(now)) return 'Today';
	if (key(d) === key(y)) return 'Yesterday';
	return new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric' }).format(d);
}

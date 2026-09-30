export function fmtDuration(ms: number, opts: { long?: boolean } = {}): string {
	if (!Number.isFinite(ms) || ms < 0) ms = 0;
	const total = Math.round(ms / 1000);
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	if (opts.long) {
		if (h) return `${h} h ${m} min`;
		if (m) return s ? `${m} min ${s} s` : `${m} min`;
		return `${s} s`;
	}
	if (h) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
	return `${m}:${String(s).padStart(2, '0')}`;
}

export function fmtBytes(n: number): string {
	if (n < 1024) return `${n} B`;
	const u = ['KB', 'MB', 'GB', 'TB'];
	let i = -1;
	do {
		n /= 1024;
		i++;
	} while (n >= 1024 && i < u.length - 1);
	return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}

export function fmtNumber(n: number, digits = 0): string {
	return n.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits });
}

export function fmtAmps(a: number): string {
	return `${a.toFixed(a < 10 ? 1 : 0)} A`;
}

export function plural(n: number, one: string, many = one + 's'): string {
	return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

export function fmtRelative(iso: string | number | Date, now = Date.now()): string {
	const t = new Date(iso).getTime();
	const d = Math.round((t - now) / 1000);
	const a = Math.abs(d);
	const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
	if (a < 45) return d >= 0 ? 'in a moment' : 'just now';
	if (a < 3600) return rtf.format(Math.round(d / 60), 'minute');
	if (a < 86400) return rtf.format(Math.round(d / 3600), 'hour');
	return rtf.format(Math.round(d / 86400), 'day');
}

/** "2 h 14 min" style countdown */
export function fmtCountdown(ms: number): string {
	if (ms <= 0) return 'now';
	const m = Math.round(ms / 60000);
	if (m < 1) return 'less than a minute';
	if (m < 60) return `${m} min`;
	const h = Math.floor(m / 60);
	if (h < 48) return `${h} h ${m % 60} min`;
	return `${Math.round(h / 24)} days`;
}

export function fmtUptime(s: number): string {
	const d = Math.floor(s / 86400);
	const h = Math.floor((s % 86400) / 3600);
	const m = Math.floor((s % 3600) / 60);
	if (d) return `${d}d ${h}h`;
	if (h) return `${h}h ${m}m`;
	return `${m}m`;
}

export function titleCase(s: string): string {
	return s.replace(/(^|[\s-])(\w)/g, (_, a, b) => a + b.toUpperCase());
}

export function clamp(v: number, lo: number, hi: number): number {
	return Math.min(hi, Math.max(lo, v));
}

export function debounce<T extends (...a: any[]) => void>(fn: T, ms: number): T {
	let t: ReturnType<typeof setTimeout> | undefined;
	return ((...a: any[]) => {
		clearTimeout(t);
		t = setTimeout(() => fn(...a), ms);
	}) as T;
}

export function hexToRgb(hex: string): [number, number, number] {
	let h = hex.replace('#', '');
	if (h.length === 3)
		h = h
			.split('')
			.map((c) => c + c)
			.join('');
	const n = parseInt(h, 16) || 0;
	return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function rgbToHex(r: number, g: number, b: number): string {
	return '#' + [r, g, b].map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');
}

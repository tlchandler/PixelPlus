// WS6 (F11): nightly reports (mirrors crates/pixelplus-daemon/src/api/reports.rs).
import type { NightReport } from '$lib/api/types';
import type { NightReportFull, ReportSummaryFull } from '$lib/insight/api';
import type { FeatureContext } from './context';
import { HttpError } from './context';

function day(offset: number) {
	return new Date(Date.now() - offset * 86400e3).toISOString().slice(0, 10);
}

/** A wobbly night of samples every 15 minutes from 17:00 to 22:00. */
function series(date: string, base: number, amp: number, seed: number): [number, number][] {
	const t0 = new Date(`${date}T17:00:00`).getTime();
	return Array.from({ length: 21 }, (_, i) => [
		t0 + i * 15 * 60e3,
		+(base + amp * Math.sin((i + seed) / 3) + (i / 20) * amp).toFixed(2)
	]);
}

function nextDay(date: string): string {
	const d = new Date(`${date}T00:00:00Z`);
	d.setUTCDate(d.getUTCDate() + 1);
	return d.toISOString().slice(0, 10);
}

function report(date: string, i: number): NightReportFull {
	const status: NightReport['status'] = i === 2 ? 'warn' : i === 5 ? 'fail' : 'ok';
	const songs = 38 + ((i * 7) % 11);
	const problems: NightReport['problems'] =
		i === 2
			? [{ level: 'warn', code: 'temp', message: 'Garage reached 58 °C', count: 1 }]
			: i === 5
				? [{ level: 'error', code: 'show', message: 'The show stopped: audio device missing', count: 1 }]
				: [];
	return {
		date,
		status,
		headline: `1 show, ${songs} songs, ${110 + i * 3} requests, ${problems.length} problems`,
		shows: [
			{ entryId: 'scweeknt01', name: 'Weeknights', startedAt: `${date}T17:15:00-06:00`, runtimeMin: 285 }
		],
		itemsPlayed: songs,
		requests: 110 + i * 3,
		topRequests: [
			{ sequenceId: 'sallwant00', name: 'All I Want for Christmas Is You', count: 21 },
			{ sequenceId: 'swizards00', name: 'Wizards in Winter', count: 14 }
		],
		problems,
		nodes: [
			{
				nodeId: 'nmain00001',
				name: 'Main Controller',
				tempMinC: 31,
				tempMaxC: 49,
				voltsMin: 12.1,
				offlineMin: 0
			},
			{
				nodeId: 'ngarage001',
				name: 'Garage',
				tempMinC: 28,
				tempMaxC: i === 2 ? 58 : 51,
				offlineMin: i === 5 ? 14 : 0,
				syncP50Ms: 0.4,
				syncP95Ms: 1.1
			}
		],
		limiter: i === 2 ? [{ nodeId: 'ngarage001', port: 2, seconds: 42 }] : [],
		suspectPixels: i === 2 ? [{ propId: 'parch00002', name: 'Arch 2', pixels: [36] }] : [],
		diskFreePct: 71,
		updates: [],
		backupAgeDays: 1,
		generatedAt: `${date}T13:00:00Z`,
		window: { from: `${date}T12:00:00Z`, to: `${nextDay(date)}T12:00:00Z` },
		runtimeMin: 285,
		games: 3,
		gameMinutes: 6,
		triggers: 12,
		restarts: 0,
		season: '🎄 Christmas',
		series: {
			tempC: [
				{ nodeId: 'nmain00001', name: 'Main Controller', points: series(date, 40, 5, i) },
				{ nodeId: 'ngarage001', name: 'Garage', points: series(date, 44, i === 2 ? 9 : 5, i + 2) }
			],
			syncMs: [{ nodeId: 'ngarage001', name: 'Garage', points: series(date, 0.7, 0.3, i + 1) }]
		}
	};
}

const summary = (r: NightReportFull): ReportSummaryFull => ({
	date: r.date,
	status: r.status,
	headline: r.headline,
	itemsPlayed: r.itemsPlayed,
	requests: r.requests,
	problems: r.problems.reduce((a, p) => a + p.count, 0),
	runtimeMin: r.runtimeMin,
	tempMaxC: r.nodes.some((n) => n.tempMaxC !== undefined)
		? Math.max(...r.nodes.flatMap((n) => (n.tempMaxC === undefined ? [] : [n.tempMaxC])))
		: undefined
});

export function register(ctx: FeatureContext) {
	const reports = Array.from({ length: 14 }, (_, i) => report(day(i + 1), i));
	const find = (d: string) => {
		const r = reports.find((x) => x.date === d);
		if (!r) throw new HttpError(404, 'not_found', 'The report for that night was not found');
		return r;
	};
	ctx.route('GET', '/reports', ({ query }) =>
		reports.slice(0, Number(query.get('limit') ?? 30)).map(summary)
	);
	ctx.route('POST', '/reports/run', ({ body }) => {
		const date = body?.date || day(1);
		let r = reports.find((x) => x.date === date);
		if (!r) {
			r = report(date, 0);
			reports.push(r);
			reports.sort((a, b) => b.date.localeCompare(a.date));
		}
		r.generatedAt = new Date().toISOString();
		const email = ctx.server.show.settings.alerts.email?.smtpHost;
		const push = ctx.server.show.settings.alerts.ntfy?.topic;
		r.delivery = body?.send
			? [
					email
						? `Email sent to ${ctx.server.show.settings.alerts.email?.to}.`
						: 'Email: not set up (Settings → Alerts).',
					push ? `Push sent to "${push}".` : 'Push: not set up (Settings → Alerts).'
				]
			: [];
		if (body?.send) ctx.log('info', `Nightly report ${r.date} sent`);
		return r;
	});
	ctx.route('GET', '/reports/(\\d{4}-\\d{2}-\\d{2})/email', ({ params }) => {
		const r = find(params[0]);
		return new Response(
			`<!doctype html><html><body style="font-family:sans-serif;background:#f4f5f7;padding:24px"><div style="background:#fff;border-radius:12px;padding:20px;max-width:600px;margin:auto"><div style="color:#666;font-size:13px">${ctx.server.show.name} · nightly report</div><h1 style="font-size:22px">${r.date}</h1><p>${r.headline}.</p><p style="color:#888;font-size:12px">(Demo preview)</p></div></body></html>`,
			{ headers: { 'content-type': 'text/html' } }
		);
	});
	ctx.route('GET', '/reports/(\\d{4}-\\d{2}-\\d{2})', ({ params }) => find(params[0]));
}

// WS6 (F11): nightly reports.
import type { NightReport, ReportSummary } from '$lib/api/types';
import type { FeatureContext } from './context';

function day(offset: number) {
	return new Date(Date.now() - offset * 86400e3).toISOString().slice(0, 10);
}

function report(date: string, i: number): NightReport {
	const status = i === 2 ? 'warn' : 'ok';
	return {
		date,
		status,
		headline: `3 shows, ${40 + i} songs, ${110 + i * 3} requests, ${i === 2 ? 1 : 0} problems`,
		shows: [
			{ entryId: 'scweeknt01', name: 'Weeknights', startedAt: `${date}T17:15:00-06:00`, runtimeMin: 285 }
		],
		itemsPlayed: 40 + i,
		requests: 110 + i * 3,
		topRequests: [{ sequenceId: 'sallwant00', name: 'All I Want for Christmas Is You', count: 21 }],
		problems: i === 2 ? [{ level: 'warn', code: 'temp', message: 'Garage reached 58 °C', count: 1 }] : [],
		nodes: [
			{ nodeId: 'nmain00001', name: 'Main Controller', tempMinC: 31, tempMaxC: 49, offlineMin: 0 },
			{
				nodeId: 'ngarage001',
				name: 'Garage',
				tempMinC: 28,
				tempMaxC: i === 2 ? 58 : 51,
				offlineMin: 0,
				syncP50Ms: 0.4,
				syncP95Ms: 1.1
			}
		],
		limiter: [],
		suspectPixels: [],
		diskFreePct: 71,
		updates: [],
		backupAgeDays: 1
	};
}

export function register(ctx: FeatureContext) {
	const reports = Array.from({ length: 7 }, (_, i) => report(day(i + 1), i));
	ctx.route('GET', '/reports', ({ query }): ReportSummary[] =>
		reports
			.slice(0, Number(query.get('limit') ?? 30))
			.map(({ date, status, headline }) => ({ date, status, headline }))
	);
	ctx.route(
		'GET',
		'/reports/(\\d{4}-\\d{2}-\\d{2})',
		({ params }) => reports.find((r) => r.date === params[0]) ?? report(params[0], 0)
	);
	ctx.route('POST', '/reports/run', ({ body }) => {
		const r = report(body?.date ?? day(0), 0);
		if (body?.send) ctx.log('info', `Report for ${r.date} sent`);
		return r;
	});
}

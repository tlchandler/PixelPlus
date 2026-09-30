// WS0 (F11): GET /journal?date=&types= — a day of the show journal.
import type { JournalRecord } from '$lib/api/types';
import type { FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	ctx.route('GET', '/journal', ({ query }) => {
		const types = query.get('types')?.split(',').filter(Boolean);
		const day = query.get('date') ?? new Date().toISOString().slice(0, 10);
		const at = (h: number, m: number) =>
			`${day}T${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:00.000-06:00`;
		const all: JournalRecord[] = [
			{ ts: at(7, 0), ev: 'restart', reason: 'start' },
			{ ts: at(17, 15), ev: 'showStart', entryId: 'scweeknt01', name: 'Weeknights' },
			{ ts: at(17, 15), ev: 'itemStart', item: 'sequence', id: 'swizards00', name: 'Wizards in Winter' },
			{
				ts: at(17, 18),
				ev: 'itemEnd',
				item: 'sequence',
				id: 'swizards00',
				name: 'Wizards in Winter',
				durMs: 185000,
				endedBy: 'finished'
			},
			{ ts: at(17, 20), ev: 'request', sequenceId: 'sallwant00', name: 'All I Want for Christmas Is You' },
			{ ts: at(19, 2), ev: 'warn', code: 'temp', msg: 'Garage is at 58 °C' },
			{ ts: at(22, 0), ev: 'showEnd', entryId: 'scweeknt01', name: 'Weeknights' }
		];
		return types?.length ? all.filter((r) => types.includes(r.ev)) : all;
	});
}

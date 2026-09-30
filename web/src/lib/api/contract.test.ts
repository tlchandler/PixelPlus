// Contract check: the real pixelplusd must answer every GET endpoint the UI reads with
// the same JSON shape as the demo backend (which the UI is built and tested against).
//
// Skipped unless PIXELPLUS_E2E_URL points at a running, set-up leader, e.g.
//   scripts/dev-cluster.sh start && node scripts/e2e/run.mjs --keep
//   PIXELPLUS_E2E_URL=http://127.0.0.1:18080 pnpm vitest run src/lib/api/contract.test.ts
import { describe, expect, it } from 'vitest';
import { MockServer } from '$lib/mock/server';
import { ENDPOINTS as FEATURE_ENDPOINTS } from './contract-endpoints';

const BASE = process.env.PIXELPLUS_E2E_URL?.replace(/\/$/, '');

/** Paths whose object keys are data (ids, effect kinds, input ids), not field names: their
 *  values are compared with each other instead of key by key. `{}` in a path is "any key". */
const MAPS = new Set([
	'/effects/schema',
	'/system/sensors/history.series',
	// F20: live state by sensor node id, then per input id.
	'/sensor-nodes/live',
	'/sensor-nodes/live{}.inputs',
	'/sensor-nodes/live{}.amps',
	'/sensor-nodes/live{}.volts'
]);

/** Keys the daemon may leave out (optional in types.ts and in the model). */
const OPTIONAL = new Set([
	'notes',
	'color',
	'layout',
	'matrix',
	'maxMilliampsPerPixel',
	'xlightsModel',
	'boardRev',
	'piModel',
	'lastSeen',
	'mediaId',
	'thumbnail',
	'xlightsName',
	'loudnessLufs',
	'gainDb',
	'musicBedMediaId',
	'eq',
	'energy',
	'label',
	'location',
	'fuseAmps',
	'dateRange',
	'idleEffectId',
	'offEffectId',
	'volumeCurfew',
	'playlist',
	'item',
	'nextItem',
	'scheduleEntry',
	'nextShow',
	'warn',
	'crit',
	'nodeId',
	'action',
	'job',
	'error',
	'problem',
	'player',
	'lastError',
	'showVersion',
	'auto',
	'notes',
	'channel',
	'unit',
	'help',
	'options',
	'min',
	'max',
	'step',
	'points',
	'channelRuns',
	'ntfy',
	'email',
	'playlistId',
	'matrixPropId',
	'units',
	'radioFrequency',
	'publicUrl',
	// Feature wave (model.rs: omitted while empty / unset).
	'measuredPixels',
	'serial',
	'hardwareHistory',
	'mainFuseAmps',
	'suspectPixels',
	'source',
	'generated',
	'tags',
	'analysis',
	'originalName',
	'originalSize',
	'smart',
	'startExact',
	'lastCalibration',
	'extraNames',
	'globalAmps',
	'globalWatts',
	'tailscale',
	'cloudflare',
	'watchFolder',
	'profiles',
	'activeProfileId',
	'profileAutoSwitch',
	'powerSupplies',
	'tagDefs',
	'sensorNodes',
	'cooldownS',
	'when',
	'activeWindow',
	'maxPerHour',
	'power',
	// Omitted while unknown: no update installed yet (F15), no temperature readings (F11).
	'previous',
	'tempMaxC',
	// F12: absent while a node's limiter is off.
	'limiter',
	// F11 night report: no backup yet / no active season.
	'backupAgeDays',
	'season'
]);

type Problem = { path: string; kind: 'missing' | 'type'; detail: string };

function kind(v: unknown): string {
	if (v === null || v === undefined) return 'null';
	if (Array.isArray(v)) return 'array';
	return typeof v;
}

/** `type`, `kind` or `ev` (journal) when every element is an object carrying it as a string. */
function discriminator(items: unknown[]): string | null {
	for (const tag of ['type', 'kind', 'ev']) {
		if (items.every((x) => kind(x) === 'object' && typeof (x as Record<string, unknown>)[tag] === 'string'))
			return tag;
	}
	return null;
}

/** Union of the keys of every object in `items` (so one sparse element doesn't hide fields). */
function mergeObjects(items: unknown[]): Record<string, unknown> | undefined {
	const objs = items.filter((x) => kind(x) === 'object') as Record<string, unknown>[];
	if (!objs.length) return undefined;
	const out: Record<string, unknown> = {};
	for (const o of objs)
		for (const [k, v] of Object.entries(o)) if (!(k in out) || kind(out[k]) === 'null') out[k] = v;
	return out;
}

export function compareShape(
	mock: unknown,
	real: unknown,
	path: string,
	out: Problem[],
	maps: ReadonlySet<string> = MAPS
) {
	const km = kind(mock);
	const kr = kind(real);
	if (km === 'null' || kr === 'null') return; // optional / nullable
	if (km !== kr) {
		out.push({ path, kind: 'type', detail: `mock ${km}, daemon ${kr}` });
		return;
	}
	if (km === 'array') {
		const m = mock as unknown[];
		const r = real as unknown[];
		if (!m.length || !r.length) return;
		// Tagged unions (playlist items, param specs): compare variant by variant.
		const tag = discriminator(m) && discriminator(r) ? discriminator(m) : null;
		if (tag) {
			const groups = new Set(m.map((x) => (x as Record<string, unknown>)[tag] as string));
			for (const g of groups) {
				const mg = m.filter((x) => (x as Record<string, unknown>)[tag] === g);
				const rg = r.filter((x) => (x as Record<string, unknown>)[tag] === g);
				if (rg.length) compareShape(mergeObjects(mg), mergeObjects(rg), `${path}[${tag}=${g}]`, out, maps);
			}
			return;
		}
		const mo = mergeObjects(m);
		const ro = mergeObjects(r);
		if (mo && ro) compareShape(mo, ro, path + '[]', out, maps);
		else compareShape(m[0], r[0], path + '[]', out, maps);
		return;
	}
	if (km !== 'object') return;
	const m = mock as Record<string, unknown>;
	const r = real as Record<string, unknown>;
	for (const tag of ['type', 'kind', 'ev']) {
		// Different variants of a tagged union (e.g. a clock vs. a sunset TimeSpec).
		if (typeof m[tag] === 'string' && typeof r[tag] === 'string' && m[tag] !== r[tag]) return;
	}
	if (path.endsWith('.params')) return; // effect parameters are per-effect data
	if (maps.has(path)) {
		const mvals = Object.values(m);
		const rvals = Object.values(r);
		// Every value of a map has one shape: primitives must agree in type, objects are merged.
		const types = (xs: unknown[]) => new Set(xs.map(kind).filter((k) => k !== 'null'));
		const [tm, tr] = [types(mvals), types(rvals)];
		if (tm.size === 1 && tr.size === 1 && [...tm][0] !== [...tr][0]) {
			out.push({ path: path + '{}', kind: 'type', detail: `mock ${[...tm][0]}, daemon ${[...tr][0]}` });
			return;
		}
		const mv = mergeObjects(mvals);
		const rv = mergeObjects(rvals);
		if (mv && rv) compareShape(mv, rv, path + '{}', out, maps);
		else if (mvals.length && rvals.length) compareShape(mvals[0], rvals[0], path + '{}', out, maps);
		return;
	}
	for (const [k, v] of Object.entries(m)) {
		if (!(k in r)) {
			if (!OPTIONAL.has(k) && v !== null && v !== undefined)
				out.push({
					path: `${path}.${k}`,
					kind: 'missing',
					detail: `daemon has no "${k}" (mock: ${kind(v)})`
				});
			continue;
		}
		compareShape(v, r[k], `${path}.${k}`, out, maps);
	}
}

const ENDPOINTS = [
	'/system',
	'/system/network',
	'/system/sensors',
	'/system/sensors/history?minutes=60',
	'/system/update',
	'/system/helpers',
	'/system/ssh',
	'/system/output-geometry',
	'/system/audio/devices',
	'/show',
	'/nodes',
	'/nodes/discovered',
	'/receivers',
	'/props',
	'/prop-groups',
	'/effects',
	'/effects/schema',
	'/playlists',
	'/dj-clips',
	'/dj-voices',
	'/sequences',
	'/media',
	'/schedule',
	'/schedule/preview?days=14',
	'/player',
	'/power/estimate',
	'/health',
	'/snapshots',
	'/tts/status',
	'/public/requests',
	'/requests',
	'/games/status',
	'/games/roms',
	'/journal',
	'/features'
];

describe.skipIf(!BASE)('daemon JSON matches the UI contract', () => {
	const mock = new MockServer({ autoplay: false });
	const fromMock = async (p: string) => {
		const r = await mock.fetch('/api/v1' + p, { method: 'GET' });
		return r.status === 200 ? r.json() : undefined;
	};
	const cookie = process.env.PIXELPLUS_E2E_COOKIE;
	const fromDaemon = async (p: string) => {
		const r = await fetch(BASE + '/api/v1' + p, { headers: cookie ? { cookie } : {} });
		expect(r.status, `${p} answered ${r.status}: ${await r.clone().text()}`).toBe(200);
		return r.json();
	};

	for (const p of [...ENDPOINTS, ...FEATURE_ENDPOINTS]) {
		it(`GET ${p}`, async () => {
			const [m, r] = await Promise.all([fromMock(p), fromDaemon(p)]);
			const path = p.split('?')[0];
			const problems: Problem[] = [];
			if (m !== undefined) compareShape(m, r, path, problems);
			expect(problems.map((x) => `${x.path}: ${x.detail}`)).toEqual([]);
		});
	}

	it('GET /media/:id/peaks and /power/estimate?sequenceId=', async () => {
		const seqs = await fromDaemon('/sequences');
		const media = await fromDaemon('/media');
		if (media.length) {
			const peaks = await fromDaemon(`/media/${media[0].id}/peaks?n=40`);
			expect(Array.isArray(peaks)).toBe(true);
			expect(peaks.length).toBeGreaterThan(0);
		}
		if (seqs.length) {
			const m = await fromMock(`/power/estimate?sequenceId=${mock.show.sequences[0].id}`);
			const r = await fromDaemon(`/power/estimate?sequenceId=${seqs[0].id}`);
			const problems: Problem[] = [];
			compareShape(m, r, '/power/estimate', problems);
			expect(problems.map((x) => `${x.path}: ${x.detail}`)).toEqual([]);
		}
	});

	it('GET /reports/:date (F11 night report)', async () => {
		const [mockList, list] = await Promise.all([
			fromMock('/reports?limit=1'),
			fromDaemon('/reports?limit=1')
		]);
		if (!list.length || !mockList?.length) return;
		const [m, r] = await Promise.all([
			fromMock(`/reports/${mockList[0].date}`),
			fromDaemon(`/reports/${list[0].date}`)
		]);
		const problems: Problem[] = [];
		compareShape(m, r, '/reports/:date', problems);
		expect(problems.map((x) => `${x.path}: ${x.detail}`)).toEqual([]);
	});
});

describe('compareShape', () => {
	it('compares map-shaped responses (keys are ids) value by value, nested maps too', () => {
		const maps = new Set(['/live', '/live{}.inputs']);
		const mock = { sn1: { online: true, rssi: -60, inputs: { pir1: 0, btn1: 1 } } };
		const same = { snAbc: { online: false, rssi: -70, inputs: { beam: 1 } } };
		const out: Problem[] = [];
		compareShape(mock, same, '/live', out, maps);
		expect(out).toEqual([]);
		compareShape(mock, { snX: { online: 'yes', inputs: { a: 'on' } } }, '/live', out, maps);
		expect(out.map((p) => `${p.path}: ${p.kind}`).sort()).toEqual([
			'/live{}.inputs{}: type',
			'/live{}.online: type',
			'/live{}.rssi: missing'
		]);
		// An empty map on either side has nothing to compare.
		const none: Problem[] = [];
		compareShape(mock, {}, '/live', none, maps);
		expect(none).toEqual([]);
	});

	it('reports missing keys and type mismatches, tolerates nulls and optional keys', () => {
		const out: Problem[] = [];
		compareShape(
			{ a: 1, b: 'x', c: [{ d: true }], notes: 'n', e: null },
			{ a: '1', c: [{}], f: 2 },
			'/t',
			out
		);
		expect(out.map((p) => p.path).sort()).toEqual(['/t.a', '/t.b', '/t.c[].d']);
	});
});

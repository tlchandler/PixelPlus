// Contract check: the real pixelplusd must answer every GET endpoint the UI reads with
// the same JSON shape as the demo backend (which the UI is built and tested against).
//
// Skipped unless PIXELPLUS_E2E_URL points at a running, set-up leader, e.g.
//   scripts/dev-cluster.sh start && node scripts/e2e/run.mjs --keep
//   PIXELPLUS_E2E_URL=http://127.0.0.1:18080 pnpm vitest run src/lib/api/contract.test.ts
import { describe, expect, it } from 'vitest';
import { MockServer } from '$lib/mock/server';

const BASE = process.env.PIXELPLUS_E2E_URL?.replace(/\/$/, '');

/** Paths whose object keys are data (ids, effect kinds), not field names. */
const MAPS = new Set(['/effects/schema', '/system/sensors/history.series']);

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
	'channelRuns'
]);

type Problem = { path: string; kind: 'missing' | 'type'; detail: string };

function kind(v: unknown): string {
	if (v === null || v === undefined) return 'null';
	if (Array.isArray(v)) return 'array';
	return typeof v;
}

/** Union of the keys of every object in `items` (so one sparse element doesn't hide fields). */
function mergeObjects(items: unknown[]): Record<string, unknown> | undefined {
	const objs = items.filter((x) => kind(x) === 'object') as Record<string, unknown>[];
	if (!objs.length) return undefined;
	const out: Record<string, unknown> = {};
	for (const o of objs) for (const [k, v] of Object.entries(o)) if (!(k in out) || kind(out[k]) === 'null') out[k] = v;
	return out;
}

export function compareShape(mock: unknown, real: unknown, path: string, out: Problem[]) {
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
		const mo = mergeObjects(m);
		const ro = mergeObjects(r);
		if (mo && ro) compareShape(mo, ro, path + '[]', out);
		else compareShape(m[0], r[0], path + '[]', out);
		return;
	}
	if (km !== 'object') return;
	const m = mock as Record<string, unknown>;
	const r = real as Record<string, unknown>;
	if (MAPS.has(path)) {
		const mv = mergeObjects(Object.values(m));
		const rv = mergeObjects(Object.values(r));
		if (mv && rv) compareShape(mv, rv, path + '{}', out);
		else {
			const a = Object.values(m)[0];
			const b = Object.values(r)[0];
			if (a !== undefined && b !== undefined) compareShape(a, b, path + '{}', out);
		}
		return;
	}
	for (const [k, v] of Object.entries(m)) {
		if (!(k in r)) {
			if (!OPTIONAL.has(k) && v !== null && v !== undefined)
				out.push({ path: `${path}.${k}`, kind: 'missing', detail: `daemon has no "${k}" (mock: ${kind(v)})` });
			continue;
		}
		compareShape(v, r[k], `${path}.${k}`, out);
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
	'/games/roms'
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

	for (const p of ENDPOINTS) {
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
});

describe('compareShape', () => {
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

import { describe, expect, it } from 'vitest';
import {
	ESSENTIALS,
	FEATURES,
	FEATURE_IDS,
	bindFeatureSource,
	dependents,
	enabledIn,
	featureForApi,
	featureForPath,
	hrefEnabled,
	isEnabled,
	itemFeature,
	normalize,
	presetDisabled,
	presetOf,
	setFeature,
	usage
} from './features';
import { buildDemoShow, buildEmptyShow } from './mock/demo';
import { MockServer } from './mock/server';
import type { Show } from './api/types';

describe('feature catalogue', () => {
	it('has unique ids, a name, a sentence and an icon for every feature', () => {
		expect(new Set(FEATURE_IDS).size).toBe(FEATURES.length);
		expect(FEATURES.length).toBe(24);
		for (const f of FEATURES) {
			expect(f.name.length).toBeGreaterThan(2);
			expect(f.description).toMatch(/\.$/);
			expect(f.icon).toBeTruthy();
			for (const r of f.requires) expect(FEATURE_IDS).toContain(r);
		}
	});

	it('documents the dependencies (same as the daemon)', () => {
		expect(dependents('phoneTrust').sort()).toEqual(['mapYard', 'soundSync']);
		expect(dependents('triggers').sort()).toEqual(['sensors', 'surprises']);
		expect(dependents('games')).toEqual([]);
	});

	it('maps pages to features', () => {
		expect(featureForPath('/games')).toBe('games');
		expect(featureForPath('/settings/seasons/')).toBe('seasons');
		expect(featureForPath('/settings/https')).toBe('phoneTrust');
		expect(featureForPath('/reports')).toBe('reports');
		expect(featureForPath('/props')).toBeUndefined();
		expect(featureForPath('/settings')).toBeUndefined();
		expect(featureForPath('/settings/features')).toBeUndefined();
		expect(featureForPath('/settings/updates')).toBeUndefined();
	});

	it('maps API paths to features like the daemon', () => {
		expect(featureForApi('GET', '/games/status')).toBe('games');
		expect(featureForApi('GET', '/public/requests')).toBe('requests');
		expect(featureForApi('GET', '/public/health')).toBeUndefined();
		expect(featureForApi('GET', '/effects')).toBeUndefined();
		expect(featureForApi('POST', '/effects')).toBe('effects');
		expect(featureForApi('GET', '/sequences/s1/preview')).toBe('layout');
		expect(featureForApi('POST', '/player/surprise')).toBe('surprises');
		expect(featureForApi('POST', '/player/surprise/stop')).toBeUndefined();
		expect(featureForApi('GET', '/features')).toBeUndefined();
		expect(featureForApi('POST', '/hooks/trigger/t1')).toBe('triggers');
		expect(featureForApi('POST', '/triggers/t1/token')).toBe('triggers');
	});

	it('knows which playlist items belong to a feature', () => {
		expect(itemFeature({ type: 'dj' })).toBe('dj');
		expect(itemFeature({ type: 'countdown' })).toBe('countdown');
		expect(itemFeature({ type: 'effect' })).toBe('effects');
		expect(itemFeature({ type: 'command', command: 'games.invite' })).toBe('games');
		expect(itemFeature({ type: 'command', command: 'overlay.text' })).toBeUndefined();
		expect(itemFeature({ type: 'sequence' })).toBeUndefined();
	});
});

describe('turning features on and off', () => {
	it('no features key means everything on (shows from before feature toggles)', () => {
		expect(FEATURE_IDS.every((id) => enabledIn({}, id))).toBe(true);
		expect(FEATURE_IDS.every((id) => enabledIn(undefined, id))).toBe(true);
		expect(presetOf(undefined)).toBe('everything');
	});

	it('turning something off turns off what needs it', () => {
		const r = setFeature([], 'phoneTrust', false);
		expect(r.disabled).toEqual(['mapYard', 'phoneTrust', 'soundSync']);
		expect(r.changed).toEqual(['mapYard', 'soundSync', 'phoneTrust']);
	});

	it('turning something on turns on what it needs', () => {
		const r = setFeature(presetDisabled('essentials'), 'surprises', true);
		expect(r.changed).toEqual(['triggers', 'surprises']);
		expect(r.disabled).toContain('sensors');
		expect(setFeature(r.disabled, 'surprises', true).changed).toEqual([]);
	});

	it('normalizes hand-made lists and keeps ids from newer versions', () => {
		expect(normalize(['triggers', 'fromTheFuture', 'games', 'games'])).toEqual([
			'fromTheFuture',
			'games',
			'sensors',
			'surprises',
			'triggers'
		]);
	});

	it('recognizes presets', () => {
		const ess = presetDisabled('essentials');
		expect(FEATURE_IDS.filter((id) => !ess.includes(id)).sort()).toEqual([...ESSENTIALS].sort());
		expect(presetOf(ess)).toBe('essentials');
		expect(presetOf([...ess, 'fromTheFuture'])).toBe('essentials');
		expect(presetOf([])).toBe('everything');
		expect(presetOf(['games'])).toBe('custom');
	});

	it('isEnabled follows the bound show', () => {
		let show: Show | null = buildDemoShow();
		bindFeatureSource(() => show);
		expect(isEnabled('games')).toBe(true);
		show = { ...show, settings: { ...show.settings, features: { disabled: ['games'] } } };
		expect(isEnabled('games')).toBe(false);
		expect(hrefEnabled('/games')).toBe(false);
		expect(hrefEnabled('/playlists')).toBe(true);
		show = null;
		expect(isEnabled('games')).toBe(true);
	});
});

describe('in-use facts', () => {
	it('counts what the demo show uses', () => {
		const u = usage(buildDemoShow(), { gameRoms: 2 });
		expect(u.dj.inUse).toBe(true);
		expect(u.dj.facts.join(' ')).toMatch(/\d+ clips/);
		expect(u.dj.facts.join(' ')).toMatch(/used in \d+ playlists?/);
		expect(u.dj.whileOff).toMatch(/skipped/);
		expect(u.games.facts).toEqual(['on for visitors', '2 ROMs']);
		expect(u.requests.inUse).toBe(true);
		expect(u.triggers.facts).toEqual(['2 triggers']);
		expect(u.layout.inUse).toBe(false);
	});

	it('a brand-new show uses nothing optional', () => {
		const u = usage(buildEmptyShow());
		const used = FEATURE_IDS.filter((id) => u[id].inUse);
		// The empty show keeps the demo's alert rules only (no channels) and no supplies.
		expect(used.filter((id) => !['reports'].includes(id))).toEqual([]);
	});

	it('singular and plural', () => {
		const show = buildEmptyShow();
		show.settings.triggers = [{ id: 't', name: 'T', kind: 'http', action: { type: 'surprise' } }];
		const u = usage(show);
		expect(u.triggers.facts).toEqual(['1 trigger']);
		expect(u.surprises.facts).toEqual(['1 surprise trigger']);
	});
});

describe('demo backend', () => {
	async function call(m: MockServer, method: string, path: string, body?: unknown) {
		const res = await m.fetch('/api/v1' + path, {
			method,
			body: body === undefined ? undefined : JSON.stringify(body)
		});
		const text = await res.text();
		return { status: res.status, data: text ? JSON.parse(text) : undefined };
	}

	it('serves GET/PUT /features and answers feature_disabled like pixelplusd', async () => {
		const m = new MockServer({ autoplay: false });
		const g = await call(m, 'GET', '/features');
		expect(g.data.features).toHaveLength(24);
		expect((await call(m, 'GET', '/games/status')).status).toBe(200);
		const p = await call(m, 'PUT', '/features', { id: 'phoneTrust', enabled: false });
		expect(p.data.changed).toEqual(['mapYard', 'soundSync', 'phoneTrust']);
		await call(m, 'PUT', '/features', { disabled: ['games', 'requests'] });
		const games = await call(m, 'GET', '/games/status');
		expect(games.status).toBe(409);
		expect(games.data.error.code).toBe('feature_disabled');
		expect(games.data.error.message).toContain('Settings → Features');
		expect((await call(m, 'GET', '/public/requests')).status).toBe(404);
		expect((await call(m, 'GET', '/show')).data.settings.features.disabled).toEqual(['games', 'requests']);
		expect((await call(m, 'PUT', '/features', { id: 'nope', enabled: true })).status).toBe(400);
	});

	it('the setup wizard can pick a preset', async () => {
		const m = new MockServer({ autoplay: false, needsSetup: true });
		await call(m, 'POST', '/system/setup', {
			role: 'leader',
			showName: 'New',
			features: { disabled: presetDisabled('essentials') }
		});
		expect(presetOf(m.show.settings.features?.disabled)).toBe('essentials');
	});
});

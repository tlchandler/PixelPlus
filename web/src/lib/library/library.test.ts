// WS2 library helpers and demo-mode endpoints (F2, F3, F18).
import { describe, expect, it } from 'vitest';
import { MockServer } from '$lib/mock/server';
import { energyGlyph, energyWord, normalizeTag, parseTags, tagColor } from './tags';
import { expandDemo } from '$lib/mock/feat/library';
import { inflate } from '$lib/preview/pppv';
import type { SmartRules } from '$lib/api/types';

async function call<T = any>(m: MockServer, method: string, path: string, body?: unknown) {
	const res = await m.fetch('/api/v1' + path, {
		method,
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	const ct = res.headers.get('content-type') ?? '';
	const data = ct.includes('json') ? await res.json() : await res.blob();
	return { status: res.status, data: data as T };
}

const rules = (r: Partial<SmartRules> = {}): SmartRules => ({
	includeTags: [],
	includeMode: 'any',
	excludeTags: [],
	noRepeatNights: 0,
	timeRules: [],
	order: 'fixed',
	pinnedFirst: [],
	pinnedLast: [],
	interleave: [],
	interleaveEvery: 0,
	...r
});

describe('tags', () => {
	it('normalize like the daemon', () => {
		expect(normalizeTag('  Kids ')).toBe('kids');
		expect(normalizeTag('Classic,  Rock')).toBe('classic rock');
		expect(normalizeTag('x'.repeat(50))).toHaveLength(32);
		expect(parseTags('Kids, classic, kids, ')).toEqual(['kids', 'classic']);
	});
	it('colours are stable and honour tag definitions', () => {
		expect(tagColor('kids', [])).toBe(tagColor('kids', []));
		expect(tagColor('kids', [{ name: 'kids', color: '#123456' }])).toBe('#123456');
		expect(energyWord(0.8)).toBe('High energy');
		expect(energyWord(0.1)).toBe('Calm');
		expect(energyGlyph(1)).toBe('█');
	});
});

describe('demo-mode library endpoints', () => {
	it('bulk tags, lists and renames tags', async () => {
		const m = new MockServer({ autoplay: false });
		const ids = m.show.sequences.slice(0, 2).map((s) => s.id);
		const r = await call(m, 'POST', '/sequences/tags', { ids, add: [' Family '] });
		expect(r.status).toBe(200);
		expect(m.show.sequences[0].tags).toContain('family');
		const tags = await call<{ name: string; sequences: number }[]>(m, 'GET', '/library/tags');
		expect(tags.data.find((t) => t.name === 'family')?.sequences).toBe(2);
		await call(m, 'PUT', '/library/tags/family', { name: 'everyone', color: '#ff8800' });
		expect(m.show.sequences[0].tags).toContain('everyone');
		expect(m.show.tagDefs?.find((d) => d.name === 'everyone')?.color).toBe('#ff8800');
		await call(m, 'DELETE', '/library/tags/everyone');
		expect(m.show.sequences[0].tags ?? []).not.toContain('everyone');
	});

	it('smart previews honour tags, length, pins and interleave', () => {
		const m = new MockServer({ autoplay: false });
		const show = m.show;
		show.sequences.forEach((s, i) => (s.tags = i % 2 ? ['kids'] : ['classic']));
		const kids = expandDemo(show, rules({ includeTags: ['kids'] }), Date.now(), 1);
		for (const it of kids.items)
			if (it.type === 'sequence')
				expect(show.sequences.find((s) => s.id === it.sequenceId)?.tags).toContain('kids');
		expect(kids.startsAt).toHaveLength(kids.items.length);
		const first = show.sequences[0];
		const pinned = expandDemo(
			show,
			rules({ pinnedFirst: [{ id: 'p', type: 'sequence', sequenceId: first.id }], targetDurationMs: 60_000 }),
			Date.now(),
			1
		);
		expect(pinned.items[0]).toMatchObject({ id: 'p' });
		const none = expandDemo(show, rules({ includeTags: ['nothing'] }), Date.now(), 1);
		expect(none.items).toHaveLength(0);
		expect(none.notes[0]).toContain('nothing');
	});

	it('auto show jobs, analysis and previews answer like the daemon', async () => {
		const m = new MockServer({ autoplay: false });
		const styles = await call<{ id: string }[]>(m, 'GET', '/autoshow/styles');
		expect(styles.data.map((s) => s.id)).toContain('classic');
		const media = m.show.media.find((x) => x.kind === 'song')!;
		const a = await call(m, 'GET', `/media/${media.id}/analysis`);
		expect(a.status).toBe(200);
		expect(a.data.energy10Hz.rms.length).toBeGreaterThan(10);
		const bad = await call(m, 'POST', '/autoshow', { mediaId: 'nope' });
		expect(bad.status).toBe(400);
		const r = await call(m, 'POST', '/autoshow/preview', { mediaId: media.id, style: 'candy', seed: 5 });
		expect(r.data.seed).toBe(5);
		expect(r.data.sequenceId).toMatch(/^tmp-/);
		const job = await call(m, 'GET', `/jobs/${r.data.jobId}`);
		expect(job.data.kind).toBe('autoshow');
		const h = await call(m, 'GET', `/sequences/${r.data.sequenceId}/preview`);
		expect(h.data.frameMs).toBe(50);
		const block = await call<Blob>(m, 'GET', `/sequences/${r.data.sequenceId}/preview/block/0`);
		const raw = await inflate(new Uint8Array(await block.data.arrayBuffer()));
		expect(raw.length).toBe(Math.min(64, h.data.frameCount) * h.data.frameBytes);
		const missing = await call(m, 'GET', '/sequences/nope/preview');
		expect(missing.status).toBe(404);
	});
});

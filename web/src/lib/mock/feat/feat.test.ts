// Feature-wave mock endpoints answer, so every workstream's UI works in demo mode (WS0).
import { describe, expect, it } from 'vitest';
import { MockServer } from '../server';
import { PENDING_ENDPOINTS } from '$lib/api/contract-endpoints';

async function call<T = any>(m: MockServer, method: string, path: string, body?: unknown) {
	const res = await m.fetch('/api/v1' + path, {
		method,
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	const text = await res.text();
	let data: T | undefined;
	try {
		data = text ? JSON.parse(text) : undefined;
	} catch {
		data = text as T;
	}
	return { status: res.status, data };
}

describe('feature-wave mock endpoints', () => {
	it('serve every pending GET endpoint', async () => {
		const m = new MockServer({ autoplay: false });
		for (const p of [...PENDING_ENDPOINTS, '/journal']) {
			const { status } = await call(m, 'GET', p);
			expect(status, p).toBe(200);
		}
	});

	it('demo show carries the feature-wave data and settings defaults', async () => {
		const m = new MockServer({ autoplay: false });
		const s = m.show;
		expect(s.settings.power?.mode).toBe('warn');
		expect(s.settings.updates?.window.from).toBe('10:00');
		expect(s.profiles?.map((p) => p.name)).toEqual(['Christmas', 'Halloween']);
		expect(s.sequences.some((x) => x.generated?.kind === 'autoShow')).toBe(true);
		expect(s.media.every((x) => x.analysis?.bpm)).toBe(true);
		const empty = new MockServer({ autoplay: false, empty: true });
		expect(empty.show.settings.reports?.time).toBe('07:00');
		expect(empty.show.profiles).toBeUndefined();
	});

	it('manual play plays once unless loop until stopped', async () => {
		const m = new MockServer({ autoplay: false });
		const pl = m.show.playlists[0];
		pl.repeat = true;
		await call(m, 'POST', '/player/play', { playlistId: pl.id });
		expect(m.play.repeat).toBe(!!m.status().scheduleEntry);
		await call(m, 'POST', '/player/play', { playlistId: pl.id, loopUntilStopped: true });
		expect(m.play.repeat).toBe(true);
	});

	it('calibration v2 returns a pseudo-random pattern; results apply the delay', async () => {
		const m = new MockServer({ autoplay: false });
		const { data } = await call<any>(m, 'POST', '/player/calibration', { on: true, pattern: 'v2', seed: 7 });
		expect(data.eventsMs).toHaveLength(32);
		const gaps = data.eventsMs.slice(1).map((t: number, i: number) => t - data.eventsMs[i]);
		expect(Math.min(...gaps)).toBeGreaterThanOrEqual(450);
		expect(Math.max(...gaps)).toBeLessThanOrEqual(870);
		const r = await call<any>(m, 'POST', '/calibration/result', {
			residualMs: 212,
			spreadMs: 4,
			matches: 30,
			apply: true
		});
		expect(r.data.outputDelayMs).toBe(212);
		expect(m.show.settings.audio.lastCalibration?.method).toBe('phone');
	});

	it('crud and flows work for profiles, supplies, mapping, pixel count and the wizard', async () => {
		const m = new MockServer({ autoplay: false });
		const p = await call<any>(m, 'POST', '/profiles/capture', { name: 'July 4th' });
		expect(p.data.name).toBe('July 4th');
		await call(m, 'POST', `/profiles/${p.data.id}/activate`, {});
		expect(m.show.activeProfileId).toBe(p.data.id);
		const ps = await call<any>(m, 'POST', '/power-supplies', { name: 'PSU', volts: 12, amps: 29 });
		expect(m.show.powerSupplies?.some((x) => x.id === ps.data.id)).toBe(true);
		const run = await call<any>(m, 'POST', '/mapping/runs', { scope: { all: true } });
		expect(run.data.plan.bitMs).toBe(200);
		expect((await call(m, 'GET', `/mapping/runs/${run.data.runId}`)).status).toBe(200);
		let step = (await call<any>(m, 'POST', '/pixelcount/start', { nodeId: 'n', output: 1, method: 'manual' }))
			.data;
		let n = 0;
		while (step.step && n++ < 12)
			step = (
				await call<any>(m, 'POST', `/pixelcount/${step.session}/answer`, {
					seen: step.step.litUntil < 48
				})
			).data;
		expect(step.count).toBe(48);
		const wiz = await call<any>(m, 'POST', '/wizard/receiver/identify-jack', { nodeId: m.show.nodes[0].id });
		expect(wiz.data.candidates.length).toBeGreaterThan(0);
		const before = m.show.receivers.length;
		await call(m, 'POST', `/wizard/receiver/${wiz.data.sessionId}/jack`, {
			jack: wiz.data.candidates[0].jack
		});
		await call(m, 'POST', `/wizard/receiver/${wiz.data.sessionId}/finish`, { receiver: { name: 'New' } });
		expect(m.show.receivers.length).toBe(before + 1);
	});
});

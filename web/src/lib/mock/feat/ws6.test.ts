// WS6 demo endpoints (seasons, reports, sensors, xLights) behave like the daemon.
import { describe, expect, it } from 'vitest';
import { MockServer } from '../server';
import { ENDPOINTS } from '$lib/api/contract-endpoints';

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
	return { status: res.status, data: data as T };
}

describe('WS6 mock endpoints', () => {
	it('serve every WS6 contract GET endpoint', async () => {
		const m = new MockServer({ autoplay: false });
		for (const p of ENDPOINTS.filter((e) => /profiles|reports|sensor|xlights/.test(e))) {
			const { status } = await call(m, 'GET', p);
			expect(status, p).toBe(200);
		}
	});

	it('seasons: capture, preview, switch and auto-switch', async () => {
		const m = new MockServer({ autoplay: false });
		const before = m.show.profiles?.length ?? 0;
		const { data: p } = await call(m, 'POST', '/profiles/capture', { name: 'Easter' });
		expect(p.name).toBe('Easter');
		expect(m.show.profiles?.length).toBe(before + 1);
		const { data: diff } = await call(m, 'GET', `/profiles/preview-switch/${p.id}`);
		expect(diff.lines.length).toBeGreaterThan(0);
		const { status, data: show } = await call(m, 'POST', `/profiles/${p.id}/activate`, { saveCurrent: true });
		expect(status).toBe(200);
		expect(show.activeProfileId).toBe(p.id);
		const { data: chip } = await call(m, 'GET', '/profiles/active');
		expect(chip.name).toBe('Easter');
		expect((await call(m, 'POST', '/profiles/nope/activate', {})).status).toBe(404);
	});

	it('reports: list, run, send and email preview', async () => {
		const m = new MockServer({ autoplay: false });
		const { data: list } = await call(m, 'GET', '/reports?limit=5');
		expect(list.length).toBe(5);
		expect(list[0].itemsPlayed).toBeGreaterThan(0);
		const { data: r } = await call(m, 'GET', `/reports/${list[0].date}`);
		expect(r.series.tempC.length).toBeGreaterThan(0);
		const { data: sent } = await call(m, 'POST', '/reports/run', { send: true });
		expect(sent.delivery.length).toBe(2);
		const { status, data: html } = await call<string>(m, 'GET', `/reports/${list[0].date}/email`);
		expect(status).toBe(200);
		expect(html).toContain('nightly report');
		expect((await call(m, 'GET', '/reports/1999-01-01')).status).toBe(404);
	});

	it('sensors: discover, adopt, edit, live and release', async () => {
		const m = new MockServer({ autoplay: false, empty: true });
		const { data: found } = await call(m, 'GET', '/sensor-nodes/discovered');
		expect(found.length).toBe(1);
		const { data: node } = await call(m, 'POST', '/sensor-nodes/adopt', { id: found[0].id });
		expect(node.adopted).toBe(true);
		expect((await call(m, 'GET', '/sensor-nodes/discovered')).data.length).toBe(0);
		const { data: edited } = await call(m, 'PUT', `/sensor-nodes/${node.id}`, { name: 'Driveway' });
		expect(edited.name).toBe('Driveway');
		const { data: live } = await call(m, 'GET', '/sensor-nodes/live');
		expect(live[node.id].online).toBe(true);
		const { data: t } = await call(m, 'POST', '/surprises/test', { action: { type: 'surprise' } });
		expect(t.ok).toBe(true);
		expect((await call(m, 'POST', `/sensor-nodes/${node.id}/release`)).status).toBe(200);
		expect(m.show.sensorNodes?.length).toBe(0);
	});

	it('xLights: status and upload password', async () => {
		const m = new MockServer({ autoplay: false });
		expect((await call(m, 'PUT', '/xlights/password', { password: 'abc' })).status).toBe(400);
		await call(m, 'PUT', '/xlights/password', { password: 'long-enough' });
		const { data: st } = await call(m, 'GET', '/xlights/status');
		expect(st.passwordSet).toBe(true);
		expect(st.addresses.length).toBeGreaterThan(0);
	});
});

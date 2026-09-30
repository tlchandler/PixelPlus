import { describe, expect, it } from 'vitest';
import { MockServer } from './server';
import type { Prop, Show } from '$lib/api/types';

async function call<T = any>(
	m: MockServer,
	method: string,
	path: string,
	body?: unknown
): Promise<{ status: number; data: T }> {
	const res = await m.fetch('/api/v1' + path, {
		method,
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	const text = await res.text();
	return { status: res.status, data: text ? JSON.parse(text) : undefined };
}

describe('mock backend', () => {
	it('serves a realistic demo show', async () => {
		const m = new MockServer({ autoplay: false });
		const { data: show } = await call<Show>(m, 'GET', '/show');
		expect(show.nodes.map((n) => n.board)).toEqual(['difftxlarge', 'difftx']);
		expect(show.props.length).toBeGreaterThanOrEqual(25);
		expect(show.props.find((p) => p.name === 'Singing Matrix')?.matrix?.width).toBe(80);
		expect(show.djVoices.map((v) => v.id)).toEqual(expect.arrayContaining(['nick', 'holly']));
		expect(show.sequences.some((s) => s.name === 'Wizards in Winter')).toBe(true);
	});

	it('CRUD bumps the show version', async () => {
		const m = new MockServer({ autoplay: false });
		const v0 = m.show.version;
		const { data: p } = await call<Prop>(m, 'POST', '/props', {
			name: 'Test Star',
			kind: 'star',
			pixelCount: 20,
			segments: [],
			groupIds: []
		});
		expect(p.id).toMatch(/^[a-z0-9]{10}$/);
		expect(m.show.version).toBe(v0 + 1);
		await call(m, 'PUT', `/props/${p.id}`, { name: 'Renamed' });
		expect(m.show.props.find((x) => x.id === p.id)?.name).toBe('Renamed');
		await call(m, 'DELETE', `/props/${p.id}`);
		expect(m.show.props.some((x) => x.id === p.id)).toBe(false);
		const missing = await call(m, 'GET', `/props/${p.id}`);
		expect(missing.status).toBe(404);
	});

	it('bulk updates and deletes props', async () => {
		const m = new MockServer({ autoplay: false });
		const [a, b] = m.show.props;
		await call(m, 'POST', '/props/bulk', {
			ops: [
				{ op: 'update', id: a.id, patch: { name: 'X' } },
				{ op: 'delete', id: b.id }
			]
		});
		expect(m.show.props[0].name).toBe('X');
		expect(m.show.props.some((p) => p.id === b.id)).toBe(false);
	});

	it('simulates playback', async () => {
		const m = new MockServer({ autoplay: false });
		m.requests = [];
		expect(m.status().state).toBe('idle');
		await call(m, 'POST', '/player/play', { playlistId: 'plmain0001' });
		const st = m.status();
		expect(st.state).toBe('playing');
		expect(st.playlist?.name).toBe('Main Show');
		expect(st.item?.name).toBe('Welcome to the show');
		await call(m, 'POST', '/player/next');
		expect(m.status().item?.name).toBe('Wizards in Winter');
		await call(m, 'POST', '/player/pause');
		expect(m.status().state).toBe('paused');
		await call(m, 'POST', '/player/stop');
		expect(m.status().state).toBe('idle');
	});

	it('lets song requests jump the queue', async () => {
		const m = new MockServer({ autoplay: false });
		await call(m, 'POST', '/player/play', { playlistId: 'plmain0001' });
		await call(m, 'POST', '/player/next');
		expect(m.status().item?.name).toBe('All I Want for Christmas Is You');
		expect(m.requests.length).toBe(0);
	});

	it('renders preview frames covering every prop', () => {
		const m = new MockServer();
		const buf = m.renderFrame();
		const total = m.show.props.reduce((n, p) => n + p.pixelCount * 3, 0);
		expect(buf.byteLength).toBe(5 + total);
		expect(new DataView(buf).getUint8(0)).toBe(0x50);
		const rgb = new Uint8Array(buf, 5);
		expect(rgb.some((v) => v > 0)).toBe(true);
	});

	it('enforces song request rules', async () => {
		const m = new MockServer({ autoplay: false });
		const seq = m.show.sequences[1].id;
		expect((await call(m, 'POST', '/public/requests', { sequenceId: seq, name: 'Max' })).status).toBe(200);
		expect((await call(m, 'POST', '/public/requests', { sequenceId: seq })).status).toBe(409);
		m.show.settings.requests.enabled = false;
		expect((await call(m, 'POST', '/public/requests', { sequenceId: m.show.sequences[2].id })).status).toBe(
			403
		);
	});

	it('runs the fault finder as a binary search', async () => {
		const m = new MockServer({ autoplay: false });
		const prop = m.show.props.find((p) => p.pixelCount === 50)!;
		let { data: step } = await call(m, 'POST', '/faultfinder/start', { propId: prop.id });
		expect(step.litTo).toBe(50);
		const bad = 37; // pretend pixel index 37 is broken
		while (!step.done)
			step = (await call(m, 'POST', `/faultfinder/${step.session}/answer`, { lit: step.litTo <= bad })).data;
		expect(step.result.pixelIndex).toBe(bad);
	});

	it('previews the schedule', async () => {
		const m = new MockServer({ autoplay: false });
		const { data } = await call<any[]>(m, 'GET', '/schedule/preview?days=14');
		expect(data.length).toBeGreaterThan(5);
		expect(data[0]).toHaveProperty('start');
		expect(data[0]).toHaveProperty('playlistId');
	});

	it('platform endpoints: health, netwatch, helper jobs, geometry', async () => {
		const m = new MockServer({ autoplay: false });
		const h = await call(m, 'GET', '/public/health');
		expect(h.data).toMatchObject({ ok: true, role: 'leader' });

		const net = await call(m, 'GET', '/system/network');
		expect(net.data.netwatch.state).toBe('online');
		// PUT echoes the config; netwatch is read-only
		const put = await call(m, 'PUT', '/system/network', { ...net.data, netwatch: { state: 'hotspot' } });
		expect(put.data.netwatch.state).toBe('online');

		const ssh = await call(m, 'PUT', '/system/ssh', { enabled: true });
		expect(ssh.data.job).toMatchObject({ verb: 'ssh-on', state: 'running' });
		expect((await call(m, 'PUT', '/system/ssh', { enabled: true })).status).toBe(409);
		const helpers = await call(m, 'GET', '/system/helpers');
		expect(helpers.data.map((j: { verb: string }) => j.verb)).toContain('ssh-on');

		expect((await call(m, 'POST', '/system/output-geometry/apply', {})).status).toBe(409);
		m.geo = {
			...m.geo,
			ok: false,
			longestString: 1234,
			canApply: true,
			targetPixels: 1300,
			message: 'too long'
		};
		const report = await call(m, 'POST', '/health/run');
		expect(report.data.checks[0]).toMatchObject({ id: 'geometry', action: 'applyOutputGeometry' });
		const apply = await call(m, 'POST', '/system/output-geometry/apply', { reboot: true });
		expect(apply.data.job).toMatchObject({ verb: 'config-txt', state: 'running' });
		const sys = await call(m, 'GET', '/system');
		expect(sys.data.outputGeometry.ok).toBe(false);
		expect(sys.data.platform.helper).toBe(true);
	});
});

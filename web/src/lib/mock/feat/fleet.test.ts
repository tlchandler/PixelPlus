// WS5 demo mode: controller replacement / transfer (F10), remote access (F14) and cluster
// updates (F15) behave like pixelplusd, so their pages work without a controller.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MockServer } from '../server';
import type { UpdateInfo } from '$lib/api/types';

async function call<T = any>(m: MockServer, method: string, path: string, body?: unknown | FormData) {
	const pending = m.fetch('/api/v1' + path, {
		method,
		body: body instanceof FormData ? body : body === undefined ? undefined : JSON.stringify(body)
	});
	// The mock answers after a short (fake-timer) delay.
	await vi.advanceTimersByTimeAsync(150);
	const res = await pending;
	const text = await res.text();
	return { status: res.status, data: (text ? JSON.parse(text) : undefined) as T };
}

describe('fleet mocks', () => {
	beforeEach(() => vi.useFakeTimers({ toFake: ['setTimeout'] }));
	afterEach(() => vi.useRealTimers());

	it('runs a cluster update to the end and records it', async () => {
		const m = new MockServer({ autoplay: false });
		const before = (await call<UpdateInfo>(m, 'GET', '/system/update')).data;
		expect(before.available).toBe(true);
		expect(before.nodes?.length).toBe(m.show.nodes.length);
		const r = await call(m, 'POST', '/system/update', { scope: 'cluster' });
		expect(r.status).toBe(200);
		expect(r.data.run.phase).toBe('staging');
		expect((await call(m, 'POST', '/system/update')).status).toBe(409);
		await vi.runAllTimersAsync();
		const after = (await call<UpdateInfo>(m, 'GET', '/system/update')).data;
		expect(after.run?.phase).toBe('done');
		expect(after.current).toBe(before.latest);
		expect(after.history?.[0].to).toBe(before.latest);
		expect(after.previous).toBe(before.current);
		// …and back.
		expect((await call(m, 'POST', '/system/update/rollback')).status).toBe(200);
		await vi.runAllTimersAsync();
		expect((await call<UpdateInfo>(m, 'GET', '/system/update')).data.current).toBe(before.current);
	});

	it('guards remote admin exposure and transfer passphrases like the daemon', async () => {
		const m = new MockServer({ autoplay: false });
		const serve = await call(m, 'POST', '/remote/tailscale/serve', { on: true });
		expect(serve.status).toBe(409);
		expect(serve.data.error.code).toBe('password_required');
		expect(
			(await call(m, 'POST', '/remote/cloudflare/hosts', { publicHost: 'lights.example.com' })).status
		).toBe(200);
		expect(m.show.settings.requests.publicUrl).toBe('https://lights.example.com/request');
		expect((await call(m, 'POST', '/system/transfer/export', { passphrase: 'short' })).status).toBe(400);
		const ok = await call(m, 'POST', '/system/transfer/export', { passphrase: 'long enough phrase' });
		expect(ok.data.url).toMatch(/^\/api\/v1\/system\/transfer\/download\//);
	});

	it('replaces a follower with a discovered controller', async () => {
		const m = new MockServer({ autoplay: false });
		const follower = m.show.nodes.find((n) => n.role === 'follower')!;
		const cand = m.discovered[0];
		cand.board = follower.board;
		const r = await call(m, 'POST', `/nodes/${follower.id}/replace`, { candidateId: cand.id });
		expect(r.status).toBe(200);
		expect(r.data.id).toBe(follower.id);
		expect(r.data.hardwareHistory.at(-1).reason).toBe('replaced');
		expect(m.discovered.some((d) => d.id === cand.id)).toBe(false);
	});

	it('restores a show from a transfer file in the setup wizard', async () => {
		const m = new MockServer({ autoplay: false, needsSetup: true });
		const form = new FormData();
		form.append('passphrase', 'mistletoe-and-wine');
		form.append('transfer', new Blob(['x']), 'show.ppxfer');
		const p = call(m, 'POST', '/system/setup', form);
		await vi.advanceTimersByTimeAsync(2000);
		const r = await p;
		expect(r.status).toBe(200);
		expect(r.data.role).toBe('leader');
		expect(r.data.restored.files).toBeGreaterThan(0);
	});
});

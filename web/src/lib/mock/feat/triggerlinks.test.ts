// Trigger-link demo endpoints behave like pixelplusd (api/hooks.rs).
import { describe, expect, it } from 'vitest';
import { MockServer } from '../server';

async function call<T = any>(m: MockServer, method: string, path: string, body?: unknown) {
	const res = await m.fetch('/api/v1' + path, {
		method,
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	const text = await res.text();
	return { status: res.status, data: (text ? JSON.parse(text) : undefined) as T };
}

describe('trigger links (mock)', () => {
	it('makes, rotates and revokes a link; settings saves keep it', async () => {
		const m = new MockServer({ autoplay: false });
		const { data: show } = await call(m, 'GET', '/show');
		const http = show.settings.triggers.find((t: { kind: string }) => t.kind === 'http');
		const gpio = show.settings.triggers.find((t: { kind: string }) => t.kind === 'gpio');
		expect((await call(m, 'POST', `/triggers/${gpio.id}/token`)).status).toBe(400);

		const { data: first } = await call(m, 'POST', `/triggers/${http.id}/token`);
		expect(first.token).toMatch(/^ppt_[\w-]{43}$/);
		expect(first.rotated).toBe(true); // the demo trigger already had one
		const hook = `/hooks/trigger/${http.id}`;
		expect((await call(m, 'POST', `${hook}?token=${first.token}`)).status).toBe(200);
		expect((await call(m, 'POST', hook)).status).toBe(401);
		expect((await call(m, 'GET', `${hook}?token=${first.token}`)).status).toBe(405);

		// A settings save can't change or drop the token.
		const t = (await call(m, 'GET', '/show')).data.settings.triggers.find(
			(x: { id: string }) => x.id === http.id
		);
		expect(t.tokenHint).toBe(first.token.slice(-4));
		expect(t.tokenHash).toBeUndefined();
		const forged = { ...t, tokenHint: 'mine', tokenHash: 'x' };
		await call(m, 'PUT', '/show/settings', { triggers: [forged, gpio] });
		const after = m.show.settings.triggers.find((x) => x.id === http.id)!;
		expect(after.tokenHint).toBe(first.token.slice(-4));
		expect((after as { tokenHash?: string }).tokenHash).toBeUndefined();

		const { data: second } = await call(m, 'POST', `/triggers/${http.id}/token`);
		expect(second.token).not.toBe(first.token);
		expect((await call(m, 'POST', `${hook}?token=${second.token}`)).status).toBe(200);
		const { data: links } = await call(m, 'GET', '/triggers/links');
		expect(links.addresses[0].kind).toBe('name');
		expect(links.links[http.id].fired).toBe(true);

		expect((await call(m, 'DELETE', `/triggers/${http.id}/token`)).status).toBe(200);
		expect((await call(m, 'POST', `${hook}?token=${second.token}`)).status).toBe(401);
		expect(m.show.settings.triggers.find((x) => x.id === http.id)!.tokenHint).toBeUndefined();
	});

	it('answers 404 feature_disabled while Buttons & triggers is off', async () => {
		const m = new MockServer({ autoplay: false });
		await call(m, 'PUT', '/features', { id: 'triggers', enabled: false });
		const r = await call(m, 'POST', '/hooks/trigger/trhass0001?token=x');
		expect([r.status, r.data.error.code]).toEqual([404, 'feature_disabled']);
	});
});

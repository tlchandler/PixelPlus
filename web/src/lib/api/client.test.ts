import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError, request, setTransport, setUnauthorizedHandler } from './client';

function json(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

afterEach(() => setTransport((i, init) => fetch(i, init)));

describe('api client', () => {
	it('prefixes /api/v1 and parses JSON', async () => {
		const calls: [string, RequestInit | undefined][] = [];
		setTransport(async (input, init) => {
			calls.push([input, init]);
			return json({ version: 7, name: 'Test' });
		});
		const show = await api.show();
		expect(show.version).toBe(7);
		expect(calls[0][0]).toBe('/api/v1/show');
		expect(calls[0][1]?.method).toBe('GET');
	});

	it('sends JSON bodies with the right method and content type', async () => {
		let seen: RequestInit | undefined;
		setTransport(async (_i, init) => {
			seen = init;
			return new Response(null, { status: 204 });
		});
		await api.setVolume(42);
		expect(seen?.method).toBe('PUT');
		expect((seen?.headers as Record<string, string>)['Content-Type']).toBe('application/json');
		expect(JSON.parse(String(seen?.body))).toEqual({ volume: 42 });
	});

	it('marks every request as coming from the app (CSRF header)', async () => {
		const seen: RequestInit[] = [];
		setTransport(async (_i, init) => {
			seen.push(init!);
			return new Response(null, { status: 204 });
		});
		await api.reboot();
		await api.show().catch(() => undefined);
		await api.submitRequest('s1');
		for (const init of seen)
			expect((init.headers as Record<string, string>)['X-PixelPlus-Request']).toBe('1');
	});

	it('turns error bodies into ApiError with code and message', async () => {
		setTransport(async () => json({ error: { code: 'not_found', message: 'No such prop' } }, 404));
		await expect(api.props.get('nope')).rejects.toMatchObject({
			status: 404,
			code: 'not_found',
			message: 'No such prop'
		});
		await expect(api.props.get('nope')).rejects.toBeInstanceOf(ApiError);
	});

	it('calls the unauthorized handler on 401 (but not for /auth)', async () => {
		const handler = vi.fn();
		setUnauthorizedHandler(handler);
		setTransport(async () => json({ error: { code: 'unauthorized', message: 'Sign in' } }, 401));
		await expect(api.show()).rejects.toBeInstanceOf(ApiError);
		expect(handler).toHaveBeenCalledTimes(1);
		await expect(api.login('x')).rejects.toBeInstanceOf(ApiError);
		expect(handler).toHaveBeenCalledTimes(1);
	});

	it('supports raw text responses', async () => {
		setTransport(async () => new Response('line 1\nline 2'));
		expect(await request<string>('GET', '/system/logs', undefined, { raw: 'text' })).toBe('line 1\nline 2');
	});

	it('uses the uploader for multipart uploads with progress', async () => {
		const progress: number[] = [];
		setTransport(
			async () => json({}),
			async (path, form, onProgress) => {
				onProgress?.(0.5);
				onProgress?.(1);
				return { path, has: form.has('fseq') };
			}
		);
		const r = await api.sequences.upload(new File(['x'], 'a.fseq'), undefined, (p) => progress.push(p));
		expect(r).toEqual({ path: '/sequences', has: true });
		expect(progress).toEqual([0.5, 1]);
	});
});

import { describe, expect, it } from 'vitest';
import { setTransport } from '$lib/api/client';
import { setSocketFactory, type SocketLike } from '$lib/api/socket';
import { app } from './app.svelte';

function json(body: unknown) {
	return new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } });
}

describe('app store', () => {
	it('catches up with a show change announced while a reload is in flight', async () => {
		let server = 3;
		let gate: Promise<void> | null = null;
		setTransport(async (url) => {
			if (url.endsWith('/show')) {
				const version = server; // what the daemon had when it answered
				if (gate) await gate;
				return json({ version, props: [] });
			}
			return json({});
		});
		const sock: SocketLike = {
			binaryType: 'arraybuffer',
			readyState: 1,
			onopen: null,
			onclose: null,
			onerror: null,
			onmessage: null,
			send() {},
			close() {}
		};
		setSocketFactory(() => sock);
		await app.reloadShow();
		expect(app.show?.version).toBe(3);
		app.connect();
		const announce = (version: number) =>
			sock.onmessage?.({ data: JSON.stringify({ type: 'show', data: { version } }) });

		// v4 is announced; the refetch is slow and answers with v4...
		let open!: () => void;
		gate = new Promise((r) => (open = r));
		server = 4;
		announce(4);
		// ...while v5 is saved and announced before that answer arrives.
		server = 5;
		announce(5);
		gate = null;
		open();
		await app.reloadShow();
		expect(app.show?.version).toBe(5);
	});
});

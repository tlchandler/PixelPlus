// Decides whether the UI talks to a real pixelplusd or to the in-browser mock backend.
import { setTransport } from './client';
import { setSocketFactory } from './socket';

const KEY = 'pp-mock';

export function mockRequested(): boolean {
	if (typeof window === 'undefined') return false;
	try {
		const q = new URLSearchParams(location.search).get('mock');
		// ?mock=empty: the demo backend with a brand-new, empty show (first-run and empty states).
		if (q === '1' || q === 'true' || q === 'empty') sessionStorage.setItem(KEY, '1');
		if (q === 'empty') sessionStorage.setItem('pp-mock-empty', '1');
		else if (q === '1' || q === 'true') sessionStorage.removeItem('pp-mock-empty');
		if (q === '0' || q === 'false') sessionStorage.removeItem(KEY);
		if (sessionStorage.getItem(KEY) === '1') return true;
	} catch {
		/* storage blocked */
	}
	return import.meta.env.VITE_MOCK === '1' || import.meta.env.VITE_MOCK === 'true';
}

async function apiReachable(): Promise<boolean> {
	try {
		const ctl = new AbortController();
		const t = setTimeout(() => ctl.abort(), 2500);
		const res = await fetch('/api/v1/system', { signal: ctl.signal });
		clearTimeout(t);
		// Vite's proxy answers 5xx when pixelplusd isn't running.
		if (res.status >= 500) return false;
		const type = res.headers.get('content-type') ?? '';
		return res.status === 401 || type.includes('json');
	} catch {
		return false;
	}
}

let mockActive = false;
export const isMock = () => mockActive;

export async function initBackend(): Promise<{ mock: boolean; auto: boolean }> {
	let want = mockRequested();
	let auto = false;
	if (!want && import.meta.env.DEV && !(await apiReachable())) {
		want = true;
		auto = true;
	}
	if (want) {
		const mock = await import('$lib/mock');
		const server = mock.getMockServer();
		setTransport(server.fetch, server.upload);
		setSocketFactory(() => server.socket());
		mockActive = true;
	}
	return { mock: want, auto };
}

export function exitMock() {
	try {
		sessionStorage.removeItem(KEY);
		sessionStorage.removeItem('pp-mock-empty');
	} catch {
		/* ignore */
	}
}

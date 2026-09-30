import { MockServer } from './server';

let server: MockServer | null = null;

/** Singleton mock backend. `?setup=1` starts it in first-run (needsSetup) state. */
export function getMockServer(): MockServer {
	if (!server) {
		let needsSetup = false;
		try {
			const q = new URLSearchParams(location.search);
			if (q.get('setup') === '1') sessionStorage.setItem('pp-mock-setup', '1');
			needsSetup = sessionStorage.getItem('pp-mock-setup') === '1';
		} catch {
			/* ignore */
		}
		server = new MockServer({ needsSetup });
	}
	return server;
}

export { MockServer };

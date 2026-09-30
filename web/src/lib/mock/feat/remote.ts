// WS5 (F14): remote access.
import type { RemoteStatus } from '$lib/api/types';
import type { FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	const st: RemoteStatus = {
		tailscale: { installed: false, state: 'NotInstalled', httpsOk: false, funnel: false },
		cloudflare: { installed: false, running: false, urls: [] },
		publicListener: false
	};
	ctx.route('GET', '/remote/status', () => st);
	ctx.route('POST', '/remote/tailscale/(install|up|serve|funnel|down)', ({ params }) => {
		const verb = params[0];
		return ctx.server.runHelper(`tailscale-${verb}`, `Tailscale: ${verb}…`, `Tailscale: ${verb} done`, () => {
			if (verb === 'install') st.tailscale.installed = true;
			if (verb === 'up')
				st.tailscale = { ...st.tailscale, state: 'Running', dnsName: 'pixelplus.tail1234.ts.net' };
			if (verb === 'serve') st.tailscale.httpsOk = true;
			if (verb === 'funnel') st.tailscale.funnel = true;
			if (verb === 'down') st.tailscale.state = 'Stopped';
		});
	});
	ctx.route('POST', '/remote/cloudflare/(install|quick|token|stop)', ({ params }) => {
		const verb = params[0];
		return ctx.server.runHelper(
			`cloudflared-${verb}`,
			`Cloudflare: ${verb}…`,
			`Cloudflare: ${verb} done`,
			() => {
				if (verb === 'install') st.cloudflare.installed = true;
				if (verb === 'quick')
					st.cloudflare = {
						...st.cloudflare,
						running: true,
						mode: 'quick',
						urls: ['https://demo-lights.trycloudflare.com']
					};
				if (verb === 'token') st.cloudflare = { ...st.cloudflare, running: true, mode: 'token' };
				if (verb === 'stop') st.cloudflare.running = false;
			}
		);
	});
	ctx.route('POST', '/remote/test', ({ body }) => ({ ok: true, url: body?.url, ms: 180 }));
}

// WS5 (F14): remote access (Tailscale / Cloudflare Tunnel) — demo of the wizards.
import type { RemoteStatus } from '$lib/api/types';
import { HttpError, type FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	const st: RemoteStatus = {
		tailscale: { installed: false, state: 'NotInstalled', httpsOk: false, funnel: false, serve: false },
		cloudflare: { installed: false, running: false, urls: [], tokenSet: false },
		publicListener: false,
		publicPort: 8081,
		passwordSet: false,
		canManage: true
	};
	const remote = () => (ctx.server.show.settings.remote ??= { publicListener: false });
	const sync = () => {
		st.passwordSet = !!ctx.server.password;
		st.publicListener = !!remote().publicListener;
		const cf = remote().cloudflare;
		st.cloudflare.publicHost = cf?.publicHost;
		st.cloudflare.adminHost = cf?.adminHost;
		st.cloudflare.tokenSet = !!cf?.tokenSet;
		return st;
	};
	const needPassword = (what: string) => {
		if (!ctx.server.password)
			throw new HttpError(
				409,
				'password_required',
				`Set a password (Settings → Security) before ${what}: the admin pages would be reachable from outside your home.`
			);
	};
	const helper = (verb: string, running: string, done: string, after?: () => void) => ({
		ok: true,
		job: ctx.server.runHelper(verb, running, done, () => {
			after?.();
			ctx.bump();
		})
	});

	ctx.route('GET', '/remote/status', () => sync());
	ctx.route('POST', '/remote/tailscale/(install|up|serve|funnel|down)', ({ params, body }) => {
		const verb = params[0];
		const on = body?.on !== false;
		if (verb === 'install')
			return helper('tailscale-install', 'Installing Tailscale…', 'Tailscale installed', () => {
				st.tailscale = { ...st.tailscale, installed: true, state: 'NeedsLogin' };
			});
		if (verb === 'up') {
			if (body?.authKey && !/^tskey-[A-Za-z0-9_-]{8,}$/.test(body.authKey))
				throw new HttpError(
					400,
					'bad_request',
					'That doesn’t look like a Tailscale auth key (tskey-auth-…).'
				);
			if (!body?.authKey) {
				st.tailscale.loginUrl = 'https://login.tailscale.com/a/demo1234';
				// Pretend the owner logs in a few seconds later.
				setTimeout(() => {
					st.tailscale = {
						...st.tailscale,
						state: 'Running',
						loginUrl: undefined,
						dnsName: 'pixelplus.tail1234.ts.net',
						httpsOk: true,
						ips: ['100.101.102.103']
					};
					ctx.bump();
				}, 6000);
				return helper('tailscale-up', 'Starting the Tailscale login…', 'Open the login link to connect');
			}
			return helper('tailscale-up', 'Joining the tailnet…', 'Connected to Tailscale', () => {
				st.tailscale = {
					...st.tailscale,
					state: 'Running',
					dnsName: 'pixelplus.tail1234.ts.net',
					httpsOk: true
				};
			});
		}
		if (verb === 'serve') {
			if (on) needPassword('managing PixelPlus over Tailscale');
			return helper(
				'tailscale-serve',
				'Setting up Tailscale HTTPS…',
				on ? 'The admin pages are on your tailnet' : 'Off the tailnet',
				() => {
					st.tailscale.serve = on;
					remote().tailscale = {
						enabled: true,
						serveAdmin: on,
						funnelPublic: st.tailscale.funnel,
						dnsName: st.tailscale.dnsName
					};
				}
			);
		}
		if (verb === 'funnel')
			return helper(
				'tailscale-funnel',
				'Publishing the song request page…',
				on ? 'The song request page is public' : 'No longer public',
				() => {
					st.tailscale.funnel = on;
					remote().publicListener = true;
					remote().tailscale = {
						enabled: true,
						serveAdmin: !!st.tailscale.serve,
						funnelPublic: on,
						dnsName: st.tailscale.dnsName
					};
					ctx.server.show.settings.requests.publicUrl = on
						? `https://${st.tailscale.dnsName}:8443/request`
						: undefined;
				}
			);
		return helper('tailscale-down', 'Disconnecting…', 'Disconnected from Tailscale', () => {
			st.tailscale = { installed: true, state: 'Stopped', httpsOk: false, funnel: false, serve: false };
			remote().tailscale = undefined;
		});
	});
	ctx.route('POST', '/remote/cloudflare/(install|quick|token|hosts|stop)', ({ params, body }) => {
		const verb = params[0];
		const hosts = () => {
			const clean = (h?: string) =>
				(h ?? '')
					.trim()
					.replace(/^https?:\/\//, '')
					.replace(/\/.*$/, '')
					.toLowerCase() || undefined;
			const publicHost = clean(body?.publicHost);
			const adminHost = clean(body?.adminHost);
			if (adminHost) needPassword('publishing the admin pages');
			remote().cloudflare = {
				mode: 'token',
				tokenSet: !!remote().cloudflare?.tokenSet,
				publicHost,
				adminHost
			};
			if (publicHost) ctx.server.show.settings.requests.publicUrl = `https://${publicHost}/request`;
		};
		if (verb === 'install')
			return helper('cloudflared-install', 'Installing cloudflared…', 'cloudflared installed', () => {
				st.cloudflare.installed = true;
			});
		if (verb === 'quick') {
			const on = body?.on !== false;
			return helper(
				'cloudflared-quick',
				'Starting a temporary public link…',
				on ? 'Temporary public link started' : 'Stopped',
				() => {
					st.cloudflare = on
						? {
								...st.cloudflare,
								running: true,
								mode: 'quick',
								urls: ['https://calm-otter-lights.trycloudflare.com']
							}
						: { ...st.cloudflare, running: false, urls: [] };
					remote().publicListener = true;
				}
			);
		}
		if (verb === 'hosts') {
			hosts();
			ctx.bump();
			return { ok: true };
		}
		if (verb === 'token') {
			if (!body?.token || String(body.token).length < 40)
				throw new HttpError(400, 'bad_request', 'That doesn’t look like a Cloudflare tunnel token.');
			hosts();
			return helper(
				'cloudflared-token',
				'Starting the Cloudflare tunnel…',
				'Cloudflare tunnel running',
				() => {
					remote().cloudflare = { ...remote().cloudflare!, mode: 'token', tokenSet: true };
					st.cloudflare = {
						...st.cloudflare,
						running: true,
						mode: 'token',
						urls: remote().cloudflare?.publicHost ? [`https://${remote().cloudflare!.publicHost}`] : []
					};
				}
			);
		}
		return helper('cloudflared-stop', 'Stopping the tunnel…', 'Cloudflare tunnels stopped', () => {
			st.cloudflare = { ...st.cloudflare, running: false, urls: [], mode: undefined };
			remote().cloudflare = undefined;
		});
	});
	ctx.route('POST', '/remote/test', ({ body }) => {
		const url = String(body?.url ?? '');
		if (!url.startsWith('https://'))
			throw new HttpError(
				400,
				'bad_request',
				'Only this controller’s own Tailscale / Cloudflare addresses can be tested.'
			);
		return { ok: true, url: url.replace(/\/?$/, '') + '/api/v1/public/health', status: 200, ms: 180 };
	});
}

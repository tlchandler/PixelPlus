// In-browser mock of pixelplusd: HTTP routes, WebSocket stream and a small playback
// simulation with live preview frames. Lets the whole UI be demoed without hardware.
import type {
	DiscoveredNode,
	EffectPreset,
	FaultStep,
	GamesStatus,
	HealthReport,
	LogLine,
	Media,
	NodeStatus,
	PlayerStatus,
	Playlist,
	PlaylistItem,
	Prop,
	Sensor,
	Show,
	Snapshot,
	SongRequest,
	SystemInfo,
	TestRequest
} from '$lib/api/types';
import { DEFAULT_EFFECT_SCHEMA, renderEffect } from '$lib/effects/render';
import { propPoints, worldBounds } from '$lib/util/geometry';
import { expandSchedule, nextShow } from '$lib/util/schedule';
import { newId } from '$lib/util/id';
import { KOKORO_VOICES } from '$lib/util/voices';
import { buildDemoShow, GARAGE, MAIN } from './demo';
import type { SocketLike } from '$lib/api/socket';

type Json = any;
type Handler = (ctx: { params: string[]; body: Json; query: URLSearchParams; form?: FormData }) => Json | Promise<Json>;

class HttpError extends Error {
	constructor(
		public status: number,
		public code: string,
		message: string
	) {
		super(message);
	}
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const clone = <T>(v: T): T => structuredClone(v);

interface PlayState {
	state: PlayerStatus['state'];
	playlistId?: string;
	queue: PlaylistItem[];
	index: number;
	startedAt: number;
	pausedAt?: number;
	/** single-item play (sequence / dj clip) */
	single?: PlaylistItem;
	effect?: EffectPreset;
	test?: TestRequest;
	testStarted?: number;
	fault?: { propId: string; lo: number; hi: number; step: number; total: number; session: string; litTo: number };
}

export class MockServer {
	show: Show;
	system: SystemInfo;
	volume = 72;
	brightness = 100;
	blackout = false;
	play: PlayState = { state: 'idle', queue: [], index: 0, startedAt: 0 };
	sockets = new Set<MockSocket>();
	snapshots: Snapshot[] = [];
	requests: SongRequest[] = [];
	discovered: DiscoveredNode[];
	logs: LogLine[] = [];
	games: GamesStatus;
	roms = [
		{ name: 'Super Mario Bros. (World).nes', sizeBytes: 40976 },
		{ name: 'Tetris (USA).nes', sizeBytes: 49168 }
	];
	password: string | null = null;
	loggedIn = true;
	started = Date.now();
	#timers: ReturnType<typeof setInterval>[] = [];
	#coords = new Map<string, { xs: Float32Array; ys: Float32Array }>();
	#coordsVersion = -1;
	#frameNo = 0;
	#routes: [string, RegExp, Handler][] = [];
	#faultSessions = 0;

	constructor(opts: { needsSetup?: boolean; autoplay?: boolean } = {}) {
		this.show = buildDemoShow();
		this.system = {
			version: '0.9.0-demo',
			nodeId: MAIN,
			role: opts.needsSetup ? 'unconfigured' : 'leader',
			hostname: 'pixelplus-main',
			board: 'difftxlarge',
			boardRev: 'A',
			piModel: 'Raspberry Pi 4 Model B Rev 1.5',
			uptimeS: 3 * 86400 + 4 * 3600,
			cpuPct: 23,
			memPct: 41,
			diskFreeMb: 21430,
			tempC: 51.2,
			ips: ['192.168.1.40', 'fd00::40'],
			time: new Date().toISOString(),
			timezone: 'America/Chicago',
			wifi: { ssid: 'Chandler-Home', signal: -54 },
			needsSetup: !!opts.needsSetup,
			passwordSet: false,
			detectedBoard: 'difftxlarge'
		};
		this.discovered = [
			{
				id: 'n3f2a9c1b0',
				name: 'pixelplus-3f2a',
				role: 'unconfigured',
				board: 'diffsmart',
				boardRev: '1.00',
				pi: 'Raspberry Pi Zero 2 W Rev 1.0',
				ver: '0.9.0',
				http: 80,
				ip: '192.168.1.57',
				adoptedBy: null
			}
		];
		this.games = { enabled: true, running: false, arcade: false, queueLength: 0, cooldownS: 0, available: true, roms: this.roms };
		const now = Date.now();
		this.snapshots = [
			{ id: 'snap000003', label: 'Before re-import', createdAt: new Date(now - 3600e3 * 5).toISOString(), sizeBytes: 182_311, showVersion: 40 },
			{ id: 'snap000002', label: 'Automatic — nightly', createdAt: new Date(now - 86400e3).toISOString(), sizeBytes: 179_004, showVersion: 37, auto: true },
			{ id: 'snap000001', label: 'Show ready for opening night', createdAt: new Date(now - 86400e3 * 6).toISOString(), sizeBytes: 171_560, showVersion: 22 }
		];
		this.requests = [
			{ id: 'rq1', sequenceId: 'sallwant00', name: 'All I Want for Christmas Is You', requestedBy: 'Emma', requestedAt: new Date(now - 120e3).toISOString() }
		];
		this.logs = [
			{ level: 'warn', message: 'Garage: difftx rev D detected — port 3 needs a 4/5-swapped lead', time: new Date(now - 3600e3).toISOString() },
			{ level: 'info', message: 'Schedule: next show "Weeknights" at sunset + 15 min', time: new Date(now - 1800e3).toISOString() }
		];
		this.#defineRoutes();
		if (opts.autoplay !== false && !opts.needsSetup) this.#startPlaylist('plmain0001', 1, 38000);
	}

	// ------------------------------------------------------------------ transport
	fetch = async (input: string, init: RequestInit = {}): Promise<Response> => {
		const url = new URL(input, 'http://mock');
		const path = url.pathname.replace(/^\/api\/v1/, '');
		const method = (init.method ?? 'GET').toUpperCase();
		let body: Json = undefined;
		let form: FormData | undefined;
		if (init.body instanceof FormData) form = init.body;
		else if (typeof init.body === 'string' && init.body) body = JSON.parse(init.body);
		await sleep(40 + Math.random() * 90);
		try {
			const out = await this.handle(method, path, body ?? {}, url.searchParams, form);
			if (out instanceof Response) return out;
			if (out === undefined) return new Response(null, { status: 204 });
			if (typeof out === 'string') return new Response(out, { status: 200, headers: { 'content-type': 'text/plain' } });
			if (out instanceof Blob) return new Response(out, { status: 200, headers: { 'content-type': out.type } });
			return new Response(JSON.stringify(out), { status: 200, headers: { 'content-type': 'application/json' } });
		} catch (e) {
			const err = e instanceof HttpError ? e : new HttpError(500, 'internal', e instanceof Error ? e.message : String(e));
			return new Response(JSON.stringify({ error: { code: err.code, message: err.message } }), {
				status: err.status,
				headers: { 'content-type': 'application/json' }
			});
		}
	};

	upload = async (path: string, form: FormData, onProgress?: (p: number) => void, method = 'POST') => {
		let size = 0;
		for (const [, v] of form.entries()) if (v instanceof Blob) size += v.size;
		const steps = Math.min(24, Math.max(6, Math.round(size / 400_000)));
		for (let i = 1; i <= steps; i++) {
			await sleep(55);
			onProgress?.(i / steps);
		}
		const res = await this.fetch('/api/v1' + path, { method, body: form });
		const text = await res.text();
		const data = text ? JSON.parse(text) : undefined;
		if (!res.ok) throw Object.assign(new Error(data?.error?.message ?? 'Upload failed'), { status: res.status });
		return data;
	};

	socket(): SocketLike {
		const s = new MockSocket(this);
		this.sockets.add(s);
		this.#ensureTimers();
		return s;
	}

	async handle(method: string, path: string, body: Json, query: URLSearchParams, form?: FormData) {
		for (const [m, re, h] of this.#routes) {
			if (m !== method) continue;
			const match = re.exec(path);
			if (match) return h({ params: match.slice(1).map(decodeURIComponent), body, query, form });
		}
		throw new HttpError(404, 'not_found', `No mock route for ${method} ${path}`);
	}

	// ------------------------------------------------------------------ helpers
	#bump() {
		this.show.version++;
		this.#broadcast({ type: 'show', data: { version: this.show.version } });
	}
	#broadcast(msg: Json) {
		const s = JSON.stringify(msg);
		for (const sock of this.sockets) sock.deliver(s);
	}
	toast(kind: 'info' | 'success' | 'warning' | 'error', message: string) {
		this.#broadcast({ type: 'toast', data: { kind, message } });
	}
	#log(level: LogLine['level'], message: string) {
		const l = { level, message, time: new Date().toISOString() };
		this.logs.unshift(l);
		if (level === 'warn' || level === 'error') this.#broadcast({ type: 'log', data: l });
	}

	#crud<K extends keyof Show>(base: string, key: K, bump = true) {
		const list = () => this.show[key] as unknown as { id: string }[];
		const find = (id: string) => {
			const e = list().find((x) => x.id === id);
			if (!e) throw new HttpError(404, 'not_found', `Not found: ${id}`);
			return e;
		};
		this.#route('GET', `${base}`, () => list());
		this.#route('GET', `${base}/([^/]+)`, ({ params }) => find(params[0]));
		this.#route('POST', `${base}`, ({ body }) => {
			const e = { ...body, id: body.id || newId() };
			list().push(e);
			if (bump) this.#bump();
			return e;
		});
		this.#route('PUT', `${base}/([^/]+)`, ({ params, body }) => {
			const e = find(params[0]);
			Object.assign(e, body, { id: e.id });
			if (bump) this.#bump();
			return e;
		});
		this.#route('DELETE', `${base}/([^/]+)`, ({ params }) => {
			const arr = list();
			const i = arr.findIndex((x) => x.id === params[0]);
			if (i < 0) throw new HttpError(404, 'not_found', 'Not found');
			arr.splice(i, 1);
			if (bump) this.#bump();
		});
	}

	#route(method: string, pattern: string, h: Handler) {
		this.#routes.push([method, new RegExp(`^${pattern}$`), h]);
	}

	// ------------------------------------------------------------------ routes
	#defineRoutes() {
		const r = this.#route.bind(this);
		// system
		r('GET', '/system', () => ({
			...this.system,
			uptimeS: this.system.uptimeS + Math.round((Date.now() - this.started) / 1000),
			time: new Date().toISOString(),
			cpuPct: Math.round(18 + Math.random() * 14),
			tempC: +(this.#sensorsNow().find((s) => s.id === 'cpu')?.value ?? 50).toFixed(1),
			passwordSet: !!this.password
		}));
		r('POST', '/system/setup', ({ body }) => {
			this.system.needsSetup = false;
			this.system.role = body.role;
			if (body.showName) this.show.name = body.showName;
			if (body.board) this.system.board = body.board;
			if (body.location) this.show.schedule.location = body.location;
			if (body.password) this.password = body.password;
			if (body.role === 'follower') this.system.leaderName = undefined;
			this.#bump();
			return this.system;
		});
		for (const a of ['reboot', 'shutdown', 'restart-service'])
			r('POST', `/system/${a}`, () => this.toast('info', `Demo mode: would ${a.replace('-', ' ')} now`));
		r('GET', '/system/logs', () =>
			[...this.logs, ...demoLogLines()]
				.map((l) => `${l.time}  ${l.level.toUpperCase().padEnd(5)}  ${l.message}`)
				.join('\n')
		);
		let network = {
			hostname: 'pixelplus-main',
			wifi: { ssid: 'Chandler-Home', country: 'US' },
			ethernet: { dhcp: true }
		};
		r('GET', '/system/network', () => network);
		r('PUT', '/system/network', ({ body }) => (network = { ...network, ...body }));
		r('GET', '/system/network/scan', async () => {
			await sleep(900);
			return [
				{ ssid: 'Chandler-Home', signal: -48, secure: true },
				{ ssid: 'Chandler-Lights-IoT', signal: -55, secure: true },
				{ ssid: 'NETGEAR42', signal: -71, secure: true },
				{ ssid: 'xfinitywifi', signal: -79, secure: false },
				{ ssid: 'Neighbors 5G', signal: -83, secure: true }
			];
		});
		r('GET', '/system/sensors', () => this.#sensorsNow());
		r('GET', '/system/sensors/history', ({ query }) => {
			const minutes = Number(query.get('minutes') ?? 60);
			const now = Date.now();
			const series: Record<string, [number, number][]> = {};
			for (const s of this.#sensorsNow()) {
				series[s.id] = Array.from({ length: 60 }, (_, i) => {
					const t = now - (minutes * 60000 * (59 - i)) / 59;
					return [t, +(s.value + Math.sin(i / 6 + s.id.length) * s.value * 0.04).toFixed(2)];
				});
			}
			return { series };
		});
		r('GET', '/system/audio/devices', () => [
			{ id: 'default', name: 'System default' },
			{ id: 'hw:CARD=Headphones', name: 'Headphone jack (line out)' },
			{ id: 'hw:CARD=Device', name: 'USB Audio Device' },
			{ id: 'hw:CARD=vc4hdmi0', name: 'HDMI 1' }
		]);
		r('POST', '/system/eeprom', ({ body }) => {
			this.toast('success', `EEPROM written: ${body.board} rev ${body.rev}`);
			return { ok: true };
		});
		r('GET', '/system/update', () => ({
			current: '0.9.0',
			latest: '0.9.2',
			available: true,
			channel: 'stable',
			notes: '• Faster sequence slicing for followers\n• Fault finder now supports reversed segments\n• Fixes a crash when a follower disconnects mid-song'
		}));
		r('POST', '/system/update', async () => {
			await sleep(1500);
			this.toast('success', 'Demo mode: update installed');
			return { ok: true };
		});

		// auth
		r('POST', '/auth/login', ({ body }) => {
			if (this.password && body.password !== this.password) throw new HttpError(401, 'bad_password', 'That password is not right');
			this.loggedIn = true;
			return { ok: true };
		});
		r('POST', '/auth/logout', () => {
			this.loggedIn = false;
		});
		r('PUT', '/auth/password', ({ body }) => {
			if (this.password && body.current !== this.password)
				throw new HttpError(403, 'bad_password', 'Current password is not right');
			this.password = body.password || null;
			this.system.passwordSet = !!this.password;
			return { ok: true };
		});

		// show
		r('GET', '/show', () => clone(this.show));
		r('PUT', '/show/name', ({ body }) => {
			this.show.name = String(body.name || this.show.name);
			this.#bump();
			return clone(this.show);
		});
		r('PUT', '/show/settings', ({ body }) => {
			const s = this.show.settings as any;
			for (const [k, v] of Object.entries(body))
				s[k] = v && typeof v === 'object' && !Array.isArray(v) ? { ...(s[k] ?? {}), ...v } : v;
			this.#bump();
			return s;
		});

		// nodes
		r('GET', '/nodes/discovered', () => this.discovered);
		r('POST', '/nodes/adopt', async ({ body }) => {
			const d = this.discovered.find((x) => x.id === body.id);
			if (!d) throw new HttpError(404, 'not_found', 'That controller is no longer announcing itself');
			await sleep(700);
			this.discovered = this.discovered.filter((x) => x.id !== d.id);
			const outs = d.board === 'diffsmart' || d.board === 'difftx' ? 4 : d.board === 'difftxlarge' ? 60 : 0;
			const node = {
				id: d.id,
				name: body.name || 'Back Yard',
				hostname: d.name,
				role: 'follower' as const,
				board: d.board,
				boardRev: d.boardRev,
				piModel: d.pi,
				adopted: true,
				outputs: Array.from({ length: outs }, (_, i) => ({
					index: i + 1,
					label: d.board === 'diffsmart' ? `Out ${i + 1}` : `Port ${i + 1}`,
					pixelType: 'ws2811' as const,
					colorOrder: 'RGB' as const,
					brightness: 100,
					gamma: 1,
					enabled: true
				}))
			};
			this.show.nodes.push(node);
			this.#bump();
			this.toast('success', `${node.name} adopted — sending its configuration`);
			return node;
		});
		r('POST', '/nodes/([^/]+)/identify', ({ params }) => {
			const n = this.show.nodes.find((x) => x.id === params[0]);
			this.toast('info', `${n?.name ?? 'Controller'} is blinking its status light`);
		});
		r('PUT', '/nodes/([^/]+)/outputs/(\\d+)', ({ params, body }) => {
			const n = this.show.nodes.find((x) => x.id === params[0]);
			const o = n?.outputs.find((x) => x.index === Number(params[1]));
			if (!o) throw new HttpError(404, 'not_found', 'Output not found');
			Object.assign(o, body, { index: o.index, label: o.label });
			this.#bump();
			return o;
		});
		this.#crud('/nodes', 'nodes');
		this.#crud('/receivers', 'receivers');

		// props
		r('POST', '/props/bulk', ({ body }) => {
			for (const op of body.ops ?? []) {
				const i = this.show.props.findIndex((p) => p.id === op.id);
				if (i < 0) continue;
				if (op.op === 'delete') this.show.props.splice(i, 1);
				else Object.assign(this.show.props[i], op.patch, { id: op.id });
			}
			this.#syncGroups();
			this.#bump();
			return this.show.props;
		});
		r('POST', '/props/reorder', ({ body }) => {
			const order = new Map<string, number>((body.ids as string[]).map((id, i) => [id, i]));
			this.show.props.sort((a, b) => (order.get(a.id) ?? 1e9) - (order.get(b.id) ?? 1e9));
			this.#bump();
		});
		this.#crud('/props', 'props');
		this.#crud('/prop-groups', 'propGroups');
		r('GET', '/effects/schema', () => DEFAULT_EFFECT_SCHEMA);
		this.#crud('/effects', 'effects');
		this.#crud('/playlists', 'playlists');
		this.#crud('/dj-voices', 'djVoices');
		r('PUT', '/pronunciations', ({ body }) => {
			this.show.pronunciations = body;
			this.#bump();
			return body;
		});
		r('POST', '/dj-clips/([^/]+)/render', async ({ params }) => {
			const c = this.show.djClips.find((x) => x.id === params[0]);
			if (!c) throw new HttpError(404, 'not_found', 'Clip not found');
			await sleep(1600);
			const words = c.lines.reduce((n, l) => n + l.text.split(/\s+/).length, 0);
			const m: Media = { id: newId(), name: `DJ — ${c.name}`, kind: 'dj', file: `media/${c.id}.mp3`, durationMs: Math.round(words * 380 / c.speed) + 800, loudnessLufs: -15 };
			this.show.media.push(m);
			c.mediaId = m.id;
			this.#bump();
			return c;
		});
		r('POST', '/dj-clips/([^/]+)/upload', ({ params }) => {
			const c = this.show.djClips.find((x) => x.id === params[0]);
			if (!c) throw new HttpError(404, 'not_found', 'Clip not found');
			const m: Media = { id: newId(), name: `DJ — ${c.name}`, kind: 'dj', file: `media/${c.id}.wav`, durationMs: 9000, loudnessLufs: -15.5 };
			this.show.media.push(m);
			c.mediaId = m.id;
			this.#bump();
			return c;
		});
		this.#crud('/dj-clips', 'djClips');

		// import
		r('POST', '/import/xlights', async ({ form }) => {
			await sleep(900);
			const f = form?.get('rgbeffects');
			const base = this.show.props.slice(0, 3).map((p) => ({ ...clone(p), id: newId(), name: `${p.name} (imported)` }));
			return {
				props: base,
				controllers: [
					{ name: 'PixelPlus-Main', suggestedNodeId: MAIN, ports: 60 },
					{ name: 'Garage pHAT', suggestedNodeId: GARAGE, ports: 4 }
				],
				warnings: [
					`Read ${f instanceof File ? f.name : 'layout'}: 3 new models, 29 unchanged.`,
					'Model "Tune To Sign" has no controller connection and was skipped.'
				]
			};
		});
		r('POST', '/import/xlights/apply', ({ body }) => {
			for (const p of body.preview.props) this.show.props.push(p);
			this.#bump();
			return clone(this.show);
		});

		// sequences & media
		r('POST', '/sequences', ({ form }) => {
			const fseq = form?.get('fseq') as File | null;
			const audio = form?.get('audio') as File | null;
			const name = (fseq?.name ?? 'New sequence').replace(/\.fseq$/i, '').replace(/[_-]+/g, ' ');
			let mediaId: string | undefined;
			if (audio) {
				const m = this.#addMedia(audio, 'song');
				mediaId = m.id;
			} else {
				const guess = this.show.media.find((m) => m.kind === 'song' && m.name.toLowerCase() === name.toLowerCase());
				mediaId = guess?.id;
			}
			const s = {
				id: newId(),
				name,
				file: `sequences/${newId()}.fseq`,
				durationMs: 150000 + Math.round(Math.random() * 90000),
				frameMs: 50,
				channelCount: this.show.props.reduce((n, p) => n + p.pixelCount * 3, 0),
				mediaId,
				xlightsName: fseq?.name,
				hash: newId() + newId()
			};
			this.show.sequences.push(s);
			this.#bump();
			return s;
		});
		this.#crud('/sequences', 'sequences');
		r('GET', '/sequences/([^/]+)/thumbnail', () => new Response(null, { status: 404 }));
		r('POST', '/media', ({ form }) => {
			const f = form?.get('file') as File;
			const m = this.#addMedia(f, (form?.get('kind') as any) ?? 'song');
			this.#bump();
			return m;
		});
		r('GET', '/media/([^/]+)/peaks', ({ params, query }) => {
			const n = Number(query.get('n') ?? 160);
			let seed = 0;
			for (const c of params[0]) seed = (seed * 31 + c.charCodeAt(0)) | 0;
			return Array.from({ length: n }, (_, i) => {
				const env = 0.55 + 0.35 * Math.sin((i / n) * Math.PI * 3 + seed) * Math.sin((i / n) * Math.PI);
				const noise = Math.abs(Math.sin(i * 12.9898 + seed) * 43758.5453) % 1;
				return +Math.min(1, Math.max(0.05, env * (0.6 + 0.4 * noise))).toFixed(3);
			});
		});
		r('GET', '/media/([^/]+)/file', () => new Response(silentWav(1.5), { headers: { 'content-type': 'audio/wav' } }));
		this.#crud('/media', 'media');

		// schedule
		r('GET', '/schedule', () => this.show.schedule);
		r('PUT', '/schedule', ({ body }) => {
			this.show.schedule = body;
			this.#bump();
			return body;
		});
		r('GET', '/schedule/preview', ({ query }) => {
			const days = Number(query.get('days') ?? 14);
			return expandSchedule(this.show.schedule, new Date(), days)
				.filter((o) => !o.overridden)
				.map(({ date, start, end, entryId, playlistId, name }) => ({ date, start, end, entryId, playlistId, name }));
		});

		// player
		r('GET', '/player', () => this.status());
		r('POST', '/player/play', ({ body }) => {
			this.blackout = false;
			if (body.playlistId) this.#startPlaylist(body.playlistId);
			else if (body.sequenceId) this.#startSingle({ id: 'single', type: 'sequence', sequenceId: body.sequenceId });
			else if (body.djClipId) this.#startSingle({ id: 'single', type: 'dj', djClipId: body.djClipId });
			else if (this.play.state === 'paused') this.#resume();
			else this.#startPlaylist(this.show.playlists[0]?.id);
			this.#pushStatus();
		});
		r('POST', '/player/stop', () => {
			this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
			this.#pushStatus();
		});
		r('POST', '/player/pause', () => {
			if (this.play.state === 'playing') {
				this.play.state = 'paused';
				this.play.pausedAt = Date.now();
			}
			this.#pushStatus();
		});
		r('POST', '/player/resume', () => {
			this.#resume();
			this.#pushStatus();
		});
		r('POST', '/player/next', () => {
			this.#advance(1);
			this.#pushStatus();
		});
		r('POST', '/player/previous', () => {
			if (this.#posMs() > 3000) this.play.startedAt = Date.now();
			else this.#advance(-1);
			this.#pushStatus();
		});
		r('POST', '/player/seek', ({ body }) => {
			const now = Date.now();
			this.play.startedAt = now - body.posMs;
			if (this.play.pausedAt) this.play.pausedAt = now;
			this.#pushStatus();
		});
		r('PUT', '/player/volume', ({ body }) => {
			this.volume = body.volume;
			this.#pushStatus();
		});
		r('PUT', '/player/brightness', ({ body }) => {
			this.brightness = body.brightness;
			this.#pushStatus();
		});
		r('POST', '/player/blackout', ({ body }) => {
			this.blackout = !!body.enabled;
			this.#pushStatus();
		});
		r('POST', '/player/effect', ({ body }) => {
			if (!body.effect) {
				if (this.play.state === 'effect') this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
			} else this.play = { state: 'effect', queue: [], index: 0, startedAt: Date.now(), effect: body.effect };
			this.#pushStatus();
		});

		// tests
		r('POST', '/test/start', ({ body }) => {
			this.play = { ...this.play, state: 'testing', test: body, testStarted: Date.now() };
			this.#pushStatus();
		});
		r('POST', '/test/stop', () => {
			if (this.play.state === 'testing') this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
			this.#pushStatus();
		});
		r('POST', '/faultfinder/start', ({ body }) => {
			const p = this.show.props.find((x) => x.id === body.propId);
			if (!p) throw new HttpError(404, 'not_found', 'Prop not found');
			const total = Math.ceil(Math.log2(p.pixelCount + 1)) + 1;
			const session = `ff${++this.#faultSessions}`;
			this.play = {
				state: 'testing',
				queue: [],
				index: 0,
				startedAt: Date.now(),
				fault: { propId: p.id, lo: 0, hi: p.pixelCount + 1, step: 1, total, session, litTo: p.pixelCount }
			};
			return this.#faultStep();
		});
		r('POST', '/faultfinder/([^/]+)/answer', ({ body }) => {
			const f = this.play.fault;
			if (!f) throw new HttpError(409, 'no_session', 'The fault finder is not running');
			if (body.lit) f.lo = f.litTo;
			else f.hi = f.litTo;
			f.step++;
			return this.#faultStep();
		});
		r('POST', '/faultfinder/stop', () => {
			this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
		});
		r('GET', '/power/estimate', () => this.#power());
		r('GET', '/health', () => this.#health());
		r('POST', '/health/run', async () => {
			await sleep(1200);
			return this.#health();
		});

		// snapshots
		r('GET', '/snapshots', () => this.snapshots);
		r('POST', '/snapshots', ({ body }) => {
			const s = { id: newId(), label: body.label || 'Manual snapshot', createdAt: new Date().toISOString(), sizeBytes: 180_000 + Math.round(Math.random() * 9000), showVersion: this.show.version };
			this.snapshots.unshift(s);
			return s;
		});
		r('POST', '/snapshots/import', ({ form }) => {
			const f = form?.get('file') as File;
			const s = { id: newId(), label: `Imported: ${f?.name ?? 'backup'}`, createdAt: new Date().toISOString(), sizeBytes: f?.size ?? 0 };
			this.snapshots.unshift(s);
			return s;
		});
		r('POST', '/snapshots/([^/]+)/restore', async ({ params }) => {
			await sleep(600);
			const s = this.snapshots.find((x) => x.id === params[0]);
			this.#bump();
			this.toast('success', `Restored “${s?.label}”`);
		});
		r('GET', '/snapshots/([^/]+)/download', () => new Response(new Blob(['demo snapshot']), { headers: { 'content-type': 'application/zstd' } }));
		r('DELETE', '/snapshots/([^/]+)', ({ params }) => {
			this.snapshots = this.snapshots.filter((s) => s.id !== params[0]);
		});

		// tts
		r('GET', '/tts/status', () => ({
			mode: 'device',
			available: true,
			voices: KOKORO_VOICES.map((v) => ({ id: v.id, name: v.name, language: v.accent === 'US' ? 'en-us' : 'en-gb', gender: v.gender }))
		}));
		r('POST', '/tts/render', async ({ body }) => {
			const words = (body.lines ?? []).reduce((n: number, l: any) => n + String(l.text).split(/\s+/).length, 0);
			await sleep(700 + words * 25);
			return new Blob([toneWav(Math.min(6, 0.8 + words * 0.25))], { type: 'audio/wav' });
		});

		// requests
		r('GET', '/public/requests', () => {
			const st = this.status();
			const rs = this.show.settings.requests;
			return {
				title: rs.title,
				message: rs.message,
				enabled: rs.enabled,
				showName: this.show.name,
				maxQueue: rs.maxQueue,
				songs: this.show.sequences.map((s) => ({ sequenceId: s.id, name: s.name, durationMs: s.durationMs })),
				queue: this.requests.map((q) => ({ id: q.id, sequenceId: q.sequenceId, name: q.name, requestedBy: q.requestedBy })),
				nowPlaying: st.item && st.state === 'playing' ? { name: st.item.name, posMs: st.posMs, durationMs: st.durationMs } : null
			};
		});
		r('POST', '/public/requests', ({ body }) => {
			const rs = this.show.settings.requests;
			if (!rs.enabled) throw new HttpError(403, 'requests_closed', 'Song requests are closed right now');
			if (this.requests.length >= rs.maxQueue) throw new HttpError(429, 'queue_full', 'The request line is full — try again in a few minutes');
			if (this.requests.some((q) => q.sequenceId === body.sequenceId))
				throw new HttpError(409, 'already_queued', 'That song is already in the line-up!');
			const s = this.show.sequences.find((x) => x.id === body.sequenceId);
			if (!s) throw new HttpError(404, 'not_found', 'Song not found');
			const q = { id: newId(), sequenceId: s.id, name: s.name, requestedBy: body.name || undefined, requestedAt: new Date().toISOString() };
			this.requests.push(q);
			this.toast('info', `New song request: ${s.name}${q.requestedBy ? ` (from ${q.requestedBy})` : ''}`);
			return { ok: true, position: this.requests.length };
		});
		r('GET', '/requests', () => this.requests);
		r('DELETE', '/requests/([^/]+)', ({ params }) => {
			this.requests = this.requests.filter((q) => q.id !== params[0]);
		});

		// alerts / integrations
		r('POST', '/alerts/test', async ({ body }) => {
			await sleep(700);
			return { ok: true, message: body.channel === 'email' ? 'Test email sent' : 'Test notification sent' };
		});
		r('POST', '/mqtt/test', async () => {
			await sleep(700);
			const m = this.show.settings.mqtt;
			return { ok: m.enabled, message: m.enabled ? `Connected to ${m.host}:${m.port}` : 'MQTT is turned off' };
		});

		// games
		r('GET', '/games/status', () => ({ ...this.games, enabled: this.show.settings.games.enabled, arcade: this.show.settings.games.arcadeMode, roms: this.roms }));
		r('POST', '/games/invite', () => this.toast('info', 'Invite is flashing on the matrix'));
		r('POST', '/games/stop', () => {
			this.games.running = false;
			this.games.player = undefined;
		});
		r('POST', '/games/test-pattern', ({ body }) => {
			const p = this.show.props.find((x) => x.id === body.propId);
			this.play = { ...this.play, state: 'testing', test: { mode: 'countPixels', target: { propIds: [body.propId] } }, testStarted: Date.now() };
			this.toast('info', `Test pattern on ${p?.name}: blue border, red top-left, green top-right`);
			setTimeout(() => {
				if (this.play.state === 'testing') this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
			}, 8000);
		});
		r('GET', '/games/roms', () => this.roms);
		r('POST', '/games/roms', ({ form }) => {
			const f = form?.get('rom') as File;
			if (!f || !/\.nes$/i.test(f.name)) throw new HttpError(400, 'bad_rom', 'Please choose a .nes file');
			this.roms.push({ name: f.name, sizeBytes: f.size });
			return this.roms;
		});
		r('DELETE', '/games/roms/([^/]+)', ({ params }) => {
			this.roms = this.roms.filter((x) => x.name !== params[0]);
		});
	}

	#addMedia(f: File, kind: Media['kind']): Media {
		const m: Media = {
			id: newId(),
			name: f.name.replace(/\.[a-z0-9]+$/i, '').replace(/[_-]+/g, ' '),
			kind,
			file: `media/${f.name}`,
			durationMs: 150000 + Math.round(Math.random() * 60000),
			loudnessLufs: -(8 + Math.random() * 8)
		};
		m.loudnessLufs = +m.loudnessLufs!.toFixed(1);
		this.show.media.push(m);
		return m;
	}

	#syncGroups() {
		const ids = new Set(this.show.props.map((p) => p.id));
		for (const g of this.show.propGroups) g.propIds = g.propIds.filter((id) => ids.has(id));
	}

	#faultStep(): FaultStep {
		const f = this.play.fault!;
		const prop = this.show.props.find((p) => p.id === f.propId)!;
		if (f.hi - f.lo <= 1 || f.step > f.total + 2) {
			const idx = f.lo;
			const done: FaultStep = {
				session: f.session,
				litFrom: 0,
				litTo: 0,
				step: f.total,
				totalSteps: f.total,
				question: '',
				done: true,
				result:
					idx >= prop.pixelCount
						? { pixelIndex: null, message: `All ${prop.pixelCount} pixels on ${prop.name} light correctly. The problem may be in the power or data lead before the first pixel.` }
						: {
								pixelIndex: idx,
								message: `Pixel ${idx + 1} is the first one that misbehaves. Check the connection between pixel ${idx} and pixel ${idx + 1} — or replace pixel ${idx + 1}.`
							}
			};
			this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
			return done;
		}
		const mid = f.step === 1 ? prop.pixelCount : Math.ceil((f.lo + f.hi) / 2);
		f.litTo = Math.min(prop.pixelCount, Math.max(f.lo + 1, mid));
		return {
			session: f.session,
			litFrom: 0,
			litTo: f.litTo,
			step: f.step,
			totalSteps: f.total,
			question: `Pixels 1–${f.litTo} of ${prop.name} should now be solid white. Do all of them look right?`
		};
	}

	#power() {
		const perProp = this.show.props.map((p) => {
			const peak = (p.pixelCount * (p.maxMilliampsPerPixel ?? 60)) / 1000;
			return { propId: p.id, peakAmps: +peak.toFixed(2), avgAmps: +(peak * 0.32).toFixed(2) };
		});
		const perOutput: { nodeId: string; output: number; peakAmps: number; avgAmps: number }[] = [];
		for (const p of this.show.props)
			for (const s of p.segments) {
				let o = perOutput.find((x) => x.nodeId === s.nodeId && x.output === s.output);
				if (!o) perOutput.push((o = { nodeId: s.nodeId, output: s.output, peakAmps: 0, avgAmps: 0 }));
				const a = (s.pixelCount * (p.maxMilliampsPerPixel ?? 60)) / 1000;
				o.peakAmps = +(o.peakAmps + a).toFixed(2);
				o.avgAmps = +(o.avgAmps + a * 0.32).toFixed(2);
			}
		const perReceiverPort = this.show.receivers.flatMap((rx) =>
			[1, 2, 3, 4].map((port) => {
				const out = perOutput.find((o) => o.nodeId === rx.nodeId && o.output === (rx.jack - 1) * 4 + port);
				return { receiverId: rx.id, port, peakAmps: out?.peakAmps ?? 0, avgAmps: out?.avgAmps ?? 0, fuseAmps: rx.fuseAmps };
			})
		);
		const warnings = perReceiverPort
			.filter((x) => x.fuseAmps && x.peakAmps > x.fuseAmps)
			.map((x) => {
				const rx = this.show.receivers.find((r) => r.id === x.receiverId)!;
				return `${rx.name} receiver port ${x.port} could draw ${x.peakAmps.toFixed(1)} A at full white — more than its ${x.fuseAmps} A fuse. Real sequences rarely hit full white, but consider power injection.`;
			});
		return { perOutput, perReceiverPort, perProp, warnings };
	}

	#health(): HealthReport {
		const unwired = this.show.props.filter((p) => !p.segments.length);
		const checks: HealthReport['checks'] = [
			{ id: 'followers', label: 'Controllers online', status: 'ok', detail: `${this.show.nodes.length} of ${this.show.nodes.length} online and in sync` },
			{ id: 'files', label: 'Sequences on followers', status: 'ok', detail: 'All sequences delivered' },
			{ id: 'audio', label: 'Audio output', status: 'ok', detail: 'Headphone jack, volume 72%' },
			{ id: 'temp', label: 'Temperatures', status: 'ok', detail: 'Highest 51 °C (Main Controller CPU)' },
			{ id: 'power', label: '12 V supply', status: 'ok', detail: '12.1 V at the transmitter' },
			{
				id: 'wiring',
				label: 'Prop wiring',
				status: unwired.length ? 'warn' : 'ok',
				detail: unwired.length ? `${unwired.map((p) => p.name).join(', ')} not wired to any port` : 'Every prop has a port'
			},
			{ id: 'port3', label: 'Garage transmitter', status: 'warn', detail: 'Rev D board: make sure port 3 uses the 4/5-swapped lead' },
			{ id: 'schedule', label: 'Schedule', status: 'ok', detail: 'Next show tonight at sunset' }
		];
		return { ok: !checks.some((c) => c.status === 'fail'), ranAt: new Date().toISOString(), checks };
	}

	#sensorsNow(): Sensor[] {
		const t = Date.now() / 1000;
		const playing = this.play.state === 'playing' || this.play.state === 'effect';
		const amps = this.blackout ? 0.4 : playing ? 9.5 + 3.5 * Math.abs(Math.sin(t / 3)) + Math.random() : 1.1;
		const volts = 12.18 - amps * 0.018 + Math.random() * 0.03;
		return [
			{ id: 'cpu', label: 'Main Controller CPU', kind: 'temperature', value: +(49 + 3 * Math.sin(t / 40) + Math.random()).toFixed(1), unit: '°C', warn: 70, crit: 80, nodeId: MAIN },
			{ id: 'board1', label: 'Transmitter board', kind: 'temperature', value: +(33 + Math.sin(t / 60) + Math.random() * 0.4).toFixed(1), unit: '°C', warn: 60, crit: 75, nodeId: MAIN },
			{ id: 'board2', label: 'Driver bank', kind: 'temperature', value: +(36 + Math.sin(t / 50) + Math.random() * 0.4).toFixed(1), unit: '°C', warn: 60, crit: 75, nodeId: MAIN },
			{ id: 'volts', label: '12 V supply', kind: 'voltage', value: +volts.toFixed(2), unit: 'V', warn: 11.4, crit: 11, nodeId: MAIN },
			{ id: 'amps', label: '12 V current', kind: 'current', value: +amps.toFixed(2), unit: 'A', warn: 18, crit: 20, nodeId: MAIN },
			{ id: 'watts', label: 'Power', kind: 'power', value: +(amps * volts).toFixed(0), unit: 'W', nodeId: MAIN },
			{ id: 'gcpu', label: 'Garage CPU', kind: 'temperature', value: +(56 + 2 * Math.sin(t / 30) + Math.random()).toFixed(1), unit: '°C', warn: 70, crit: 80, nodeId: GARAGE }
		];
	}

	#nodesNow(): NodeStatus[] {
		const now = new Date().toISOString();
		return this.show.nodes.map((n, i) => ({
			id: n.id,
			name: n.name,
			online: true,
			lastSeen: now,
			board: n.board,
			syncOffsetMs: n.role === 'leader' ? 0 : +(0.4 + Math.random() * 1.6).toFixed(1),
			syncState: i > 1 && Date.now() - this.started < 20000 ? 'syncing' : 'synced',
			files: { pending: i > 1 && Date.now() - this.started < 20000 ? 3 : 0, total: this.show.sequences.length }
		}));
	}

	// ------------------------------------------------------------------ playback
	#itemDuration(it?: PlaylistItem): number {
		if (!it) return 0;
		switch (it.type) {
			case 'sequence':
				return this.show.sequences.find((s) => s.id === it.sequenceId)?.durationMs ?? 60000;
			case 'dj': {
				const c = this.show.djClips.find((d) => d.id === it.djClipId);
				return this.show.media.find((m) => m.id === c?.mediaId)?.durationMs ?? 12000;
			}
			case 'media':
				return this.show.media.find((m) => m.id === it.mediaId)?.durationMs ?? 10000;
			case 'effect':
			case 'pause':
				return it.durationMs;
			case 'command':
				return 3000;
		}
	}

	itemName(it?: PlaylistItem): string {
		if (!it) return '';
		switch (it.type) {
			case 'sequence':
				return this.show.sequences.find((s) => s.id === it.sequenceId)?.name ?? 'Sequence';
			case 'dj':
				return this.show.djClips.find((s) => s.id === it.djClipId)?.name ?? 'DJ clip';
			case 'media':
				return this.show.media.find((s) => s.id === it.mediaId)?.name ?? 'Audio';
			case 'effect':
				return this.show.effects.find((s) => s.id === it.effectId)?.name ?? 'Effect';
			case 'pause':
				return `Pause ${Math.round(it.durationMs / 1000)} s`;
			case 'command':
				return it.command === 'games.invite' ? 'Show game invite' : it.command;
		}
	}

	#startPlaylist(id?: string, index = 0, posMs = 0) {
		const pl = this.show.playlists.find((p) => p.id === id);
		if (!pl) throw new HttpError(404, 'not_found', 'Playlist not found');
		const queue = [...pl.intro, ...(pl.shuffle ? shuffle(pl.items) : pl.items), ...pl.outro];
		this.play = { state: 'playing', playlistId: pl.id, queue, index: Math.min(index, queue.length - 1), startedAt: Date.now() - posMs };
	}
	#startSingle(it: PlaylistItem) {
		this.play = { state: 'playing', queue: [it], index: 0, startedAt: Date.now(), single: it };
	}
	#resume() {
		if (this.play.state === 'paused' && this.play.pausedAt) {
			this.play.startedAt += Date.now() - this.play.pausedAt;
			this.play.pausedAt = undefined;
			this.play.state = 'playing';
		}
	}
	#posMs() {
		const p = this.play;
		if (p.state === 'paused' && p.pausedAt) return p.pausedAt - p.startedAt;
		return Date.now() - p.startedAt;
	}
	#advance(dir: number) {
		const p = this.play;
		if (!p.queue.length) return;
		let i = p.index + dir;
		const pl = this.show.playlists.find((x) => x.id === p.playlistId);
		if (i >= p.queue.length) {
			if (pl?.repeat) i = pl.intro.length; // loop items (skip intro)
			else {
				this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
				return;
			}
		}
		if (i < 0) i = 0;
		// Song requests jump the queue.
		if (dir > 0 && this.requests.length) {
			const rq = this.requests.shift()!;
			p.queue.splice(i, 0, { id: 'req-' + rq.id, type: 'sequence', sequenceId: rq.sequenceId });
		}
		p.index = i;
		p.startedAt = Date.now();
		if (p.pausedAt) p.pausedAt = Date.now();
	}

	status(): PlayerStatus {
		const p = this.play;
		const pl = this.show.playlists.find((x) => x.id === p.playlistId);
		const it = p.queue[p.index];
		const nx = p.queue[p.index + 1] ?? (pl?.repeat ? p.queue[pl.intro.length] : undefined);
		const ns = nextShow(this.show.schedule, new Date());
		const active = ns && new Date(ns.start) <= new Date();
		const base: PlayerStatus = {
			state: p.state,
			posMs: 0,
			durationMs: 0,
			volume: this.volume,
			brightness: this.brightness,
			fps: p.state === 'idle' ? 0 : 40,
			blackout: this.blackout,
			nextShow: ns && !active ? { name: ns.name, startsAt: ns.start } : undefined,
			scheduleEntry: ns && active ? { id: ns.entryId, name: ns.name, endsAt: ns.end } : undefined
		};
		if (p.state === 'effect' && p.effect) return { ...base, item: { type: 'effect', id: p.effect.id, name: p.effect.name }, posMs: Date.now() - p.startedAt };
		if (p.state === 'testing') return { ...base, item: { type: 'test', id: 'test', name: p.fault ? 'Fault finder' : 'Test pattern' } };
		if (!it || p.state === 'idle') return base;
		return {
			...base,
			playlist: pl && !p.single ? { id: pl.id, name: pl.name, index: p.index, count: p.queue.length } : undefined,
			item: { type: it.type, id: (it as any).sequenceId ?? (it as any).djClipId ?? (it as any).effectId ?? it.id, name: this.itemName(it) },
			posMs: Math.max(0, Math.min(this.#posMs(), this.#itemDuration(it))),
			durationMs: this.#itemDuration(it),
			nextItem: nx && !p.single ? { type: nx.type, id: nx.id, name: this.itemName(nx) } : undefined
		};
	}

	#pushStatus() {
		this.#broadcast({ type: 'status', data: this.status() });
	}

	#ensureTimers() {
		if (this.#timers.length) return;
		let tick = 0;
		this.#timers.push(
			setInterval(() => {
				tick++;
				// advance playback
				if (this.play.state === 'playing') {
					const it = this.play.queue[this.play.index];
					if (this.#posMs() >= this.#itemDuration(it)) {
						if (this.play.single) this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
						else this.#advance(1);
					}
				}
				if (this.play.state === 'testing' && this.play.test && this.play.testStarted && Date.now() - this.play.testStarted > 60000)
					this.play = { state: 'idle', queue: [], index: 0, startedAt: 0 };
				if (this.games.cooldownS > 0 && tick % 4 === 0) this.games.cooldownS--;
				const idle = this.play.state === 'idle';
				if (!idle || tick % 8 === 0) this.#pushStatus();
				if (tick % 8 === 0) this.#broadcast({ type: 'nodes', data: this.#nodesNow() });
				if (tick % 20 === 1) this.#broadcast({ type: 'sensors', data: this.#sensorsNow() });
			}, 250)
		);
	}

	onSocketOpen(s: MockSocket) {
		s.deliver(JSON.stringify({ type: 'status', data: this.status() }));
		s.deliver(JSON.stringify({ type: 'nodes', data: this.#nodesNow() }));
		s.deliver(JSON.stringify({ type: 'sensors', data: this.#sensorsNow() }));
		s.deliver(JSON.stringify({ type: 'show', data: { version: this.show.version } }));
	}

	// ------------------------------------------------------------------ preview
	#ensureCoords() {
		if (this.#coordsVersion === this.show.version) return;
		this.#coordsVersion = this.show.version;
		this.#coords.clear();
		const b = worldBounds(this.show.props);
		for (const p of this.show.props) {
			const pts = propPoints(p);
			const n = p.pixelCount;
			const xs = new Float32Array(n);
			const ys = new Float32Array(n);
			const l = p.layout ?? { x: b.x, y: b.y, w: b.w, h: b.h, rotation: 0 };
			for (let i = 0; i < n; i++) {
				xs[i] = (l.x + pts[i * 2] * l.w - b.x) / b.w;
				ys[i] = (l.y + pts[i * 2 + 1] * l.h - b.y) / b.h;
			}
			this.#coords.set(p.id, { xs, ys });
		}
	}

	renderFrame(): ArrayBuffer {
		this.#ensureCoords();
		const props = this.show.props;
		const total = props.reduce((n, p) => n + p.pixelCount * 3, 0);
		const buf = new ArrayBuffer(5 + total);
		const view = new DataView(buf);
		view.setUint8(0, 0x50);
		view.setUint32(1, ++this.#frameNo, true);
		const rgb = new Uint8Array(buf, 5);
		if (this.blackout) return buf;
		const t = (Date.now() - this.started) / 1000;
		const p = this.play;
		let off = 0;
		const it = p.queue[p.index];
		const idleFx = this.show.effects.find((e) => e.id === this.show.schedule.idleEffectId);
		for (const prop of props) {
			const n = prop.pixelCount;
			const c = this.#coords.get(prop.id)!;
			if (p.state === 'testing') this.#renderTest(prop, rgb, off, t);
			else if (p.state === 'effect' && p.effect) {
				if (targets(this.show, p.effect.target, prop)) renderEffect(p.effect.effect, p.effect.params, t, n, rgb, off, { ...c, seed: 3 });
			} else if ((p.state === 'playing' || p.state === 'paused') && it) {
				const tt = p.state === 'paused' ? (p.pausedAt! - this.started) / 1000 : t;
				if (it.type === 'sequence') this.#renderSequence(it.sequenceId, prop, rgb, off, tt, this.#posMs(), c);
				else if (it.type === 'effect') {
					const fx = this.show.effects.find((e) => e.id === it.effectId);
					if (fx) renderEffect(fx.effect, fx.params, tt, n, rgb, off, { ...c, seed: 5 });
				} else if (idleFx) renderEffect(idleFx.effect, { ...idleFx.params, brightness: 35 }, tt, n, rgb, off, { ...c, seed: 1 });
			} else if (idleFx && this.show.schedule.enabled) {
				renderEffect(idleFx.effect, { ...idleFx.params, brightness: 30 }, t, n, rgb, off, { ...c, seed: 1 });
			}
			off += n * 3;
		}
		if (this.brightness < 100) {
			const k = this.brightness / 100;
			for (let i = 0; i < rgb.length; i++) rgb[i] = rgb[i] * k;
		}
		return buf;
	}

	#renderTest(prop: Prop, rgb: Uint8Array, off: number, t: number) {
		const p = this.play;
		const n = prop.pixelCount;
		if (p.fault) {
			if (prop.id !== p.fault.propId) return;
			for (let i = 0; i < Math.min(n, p.fault.litTo); i++) rgb.fill(230, off + i * 3, off + i * 3 + 3);
			return;
		}
		const req = p.test;
		if (!req) return;
		const tg = req.target;
		const hit =
			tg.all ||
			tg.propIds?.includes(prop.id) ||
			tg.groupIds?.some((g) => prop.groupIds.includes(g)) ||
			(tg.nodeId && prop.segments.some((s) => s.nodeId === tg.nodeId && (tg.output == null || s.output === tg.output)));
		if (!hit) return;
		const [r, g, b] = hexRgb(req.color ?? '#ffffff');
		for (let i = 0; i < n; i++) {
			let c: [number, number, number] = [r, g, b];
			if (req.mode === 'rgbCycle') c = [[255, 0, 0], [0, 255, 0], [0, 0, 255]][Math.floor(t) % 3] as any;
			else if (req.mode === 'chase') c = (i + Math.floor(t * 10)) % 6 < 2 ? [r, g, b] : [0, 0, 0];
			else if (req.mode === 'walk') c = i === Math.floor(t * 8) % n ? [255, 255, 255] : [0, 0, 0];
			else if (req.mode === 'countPixels') c = i % 10 === 9 ? [255, 40, 40] : i % 5 === 4 ? [40, 255, 40] : [40, 40, 255];
			rgb[off + i * 3] = c[0];
			rgb[off + i * 3 + 1] = c[1];
			rgb[off + i * 3 + 2] = c[2];
		}
	}

	#renderSequence(seqId: string, prop: Prop, rgb: Uint8Array, off: number, t: number, posMs: number, c: { xs: Float32Array; ys: Float32Array }) {
		let h = 0;
		for (const ch of seqId) h = (h * 31 + ch.charCodeAt(0)) | 0;
		const section = Math.floor(posMs / 7000);
		const programs: [EffectPreset['effect'], Record<string, unknown>][] = [
			['wave', { colors: ['#ff2244', '#1133ff', '#ffffff'], wavelength: 0.35, speed: 0.6 }],
			['chase', { colors: ['#ff1a1a', '#18c24a', '#ffffff'], size: 6, gap: 2, speed: 22 }],
			['rainbow', { speed: 0.5, spread: 1.3, mode: 'across' }],
			['twinkle', { colors: ['#ffffff', '#a8d8ff'], density: 0.6, speed: 2, glow: 0.12 }],
			['meteor', { colors: ['#9fe3ff'], tailLength: 25, speed: 70, count: 2 }],
			['candycane', { colors: ['#ff1111', '#ffffff'], speed: 10, stripeWidth: 4 }],
			['sparkle', { colors: ['#401060'], sparkleColor: '#ffffff', density: 0.15, speed: 2 }],
			['colorwash', { colors: ['#ff2a2a', '#1fbf4f', '#ffd700'], speed: 0.3, spread: 0.6 }],
			['fire', { height: 0.9, speed: 1.4 }]
		];
		const [kind, params] = programs[Math.abs(h + section * 7) % programs.length];
		const beat = 0.55 + 0.45 * Math.pow(Math.max(0, Math.cos((posMs / 500) * Math.PI)), 6);
		if (prop.kind === 'matrix' && prop.matrix) {
			this.#renderMatrix(prop, rgb, off, t, posMs, h);
			return;
		}
		renderEffect(kind, { ...params, brightness: 100 * beat }, t, prop.pixelCount, rgb, off, { ...c, seed: h });
	}

	#renderMatrix(prop: Prop, rgb: Uint8Array, off: number, t: number, posMs: number, h: number) {
		const { width: w, height: hh, pixelMap } = prop.matrix!;
		const bars = 20;
		const bw = w / bars;
		for (let y = 0; y < hh; y++)
			for (let x = 0; x < w; x++) {
				const pi = pixelMap[y * w + x];
				if (pi < 0) continue;
				const b = Math.floor(x / bw);
				const lvl =
					0.25 +
					0.6 * Math.abs(Math.sin(posMs / (260 + b * 17) + b * 1.7 + h)) * (0.6 + 0.4 * Math.abs(Math.sin(posMs / 900 + b)));
				const top = hh - lvl * hh;
				const j = off + pi * 3;
				if (y >= top && x % bw < bw - 1) {
					const hue = (b / bars + t * 0.05) % 1;
					const [r, g, bl] = hsvRgb(hue, 0.85, 0.35 + 0.65 * ((y - top) / Math.max(1, hh - top)));
					rgb[j] = r;
					rgb[j + 1] = g;
					rgb[j + 2] = bl;
				} else {
					rgb[j] = 2;
					rgb[j + 1] = 2;
					rgb[j + 2] = 8;
				}
			}
	}
}

function targets(show: Show, t: EffectPreset['target'], p: Prop): boolean {
	if (!t || t.all) return true;
	if (t.propIds?.includes(p.id)) return true;
	if (t.groupIds?.some((g) => p.groupIds.includes(g) || show.propGroups.find((x) => x.id === g)?.propIds.includes(p.id))) return true;
	return !t.propIds?.length && !t.groupIds?.length;
}

function hexRgb(h: string): [number, number, number] {
	const n = parseInt(h.replace('#', ''), 16) || 0;
	return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function hsvRgb(h: number, s: number, v: number): [number, number, number] {
	const i = Math.floor(h * 6);
	const f = h * 6 - i;
	const p = v * (1 - s),
		q = v * (1 - f * s),
		t = v * (1 - (1 - f) * s);
	const m = [
		[v, t, p],
		[q, v, p],
		[p, v, t],
		[p, q, v],
		[t, p, v],
		[v, p, q]
	][i % 6];
	return [m[0] * 255, m[1] * 255, m[2] * 255];
}

function shuffle<T>(a: T[]): T[] {
	const b = [...a];
	for (let i = b.length - 1; i > 0; i--) {
		const j = Math.floor(Math.random() * (i + 1));
		[b[i], b[j]] = [b[j], b[i]];
	}
	return b;
}

function wavHeader(samples: number, rate: number): DataView {
	const buf = new ArrayBuffer(44 + samples * 2);
	const v = new DataView(buf);
	const w = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
	w(0, 'RIFF');
	v.setUint32(4, 36 + samples * 2, true);
	w(8, 'WAVE');
	w(12, 'fmt ');
	v.setUint32(16, 16, true);
	v.setUint16(20, 1, true);
	v.setUint16(22, 1, true);
	v.setUint32(24, rate, true);
	v.setUint32(28, rate * 2, true);
	v.setUint16(32, 2, true);
	v.setUint16(34, 16, true);
	w(36, 'data');
	v.setUint32(40, samples * 2, true);
	return v;
}

function silentWav(seconds: number): ArrayBuffer {
	return wavHeader(Math.round(8000 * seconds), 8000).buffer as ArrayBuffer;
}

/** A soft "speech-like" warble so audition buttons produce sound in demo mode. */
function toneWav(seconds: number): ArrayBuffer {
	const rate = 16000;
	const n = Math.round(rate * seconds);
	const v = wavHeader(n, rate);
	for (let i = 0; i < n; i++) {
		const t = i / rate;
		const syll = Math.max(0, Math.sin(t * Math.PI * 4.2)) ** 0.6;
		const f = 180 + 40 * Math.sin(t * 5);
		const s = (Math.sin(2 * Math.PI * f * t) * 0.5 + Math.sin(4 * Math.PI * f * t) * 0.2) * syll * 0.25;
		const fade = Math.min(1, t * 10, (seconds - t) * 10);
		v.setInt16(44 + i * 2, s * fade * 32767, true);
	}
	return v.buffer as ArrayBuffer;
}

function demoLogLines(): LogLine[] {
	const now = Date.now();
	const lines: [string, LogLine['level'], string][] = [
		['info', 'info', 'pixelplusd 0.9.0 starting (board difftxlarge rev A, Pi 4 Model B)'],
		['info', 'info', 'Output: DPI 24-bit @ 38.4 MHz, 3 latch banks, 60 outputs'],
		['info', 'info', 'Cluster: follower "Garage" (pixelplus-garage) online, offset 0.8 ms'],
		['info', 'info', 'Audio: hw:CARD=Headphones, 48 kHz, normalization to -14 LUFS'],
		['info', 'info', 'Scheduler: playlist "Main Show" started by "Weeknights"'],
		['warn', 'warn', 'Garage CPU at 61 °C'],
		['info', 'info', 'Snapshot "Automatic — nightly" saved (179 KB)']
	];
	return lines.map(([, level, message], i) => ({ level, message, time: new Date(now - (i + 2) * 3600e3).toISOString() }));
}

export class MockSocket implements SocketLike {
	binaryType: BinaryType = 'arraybuffer';
	readyState = 0;
	onopen: ((ev: any) => void) | null = null;
	onclose: ((ev: any) => void) | null = null;
	onerror: ((ev: any) => void) | null = null;
	onmessage: ((ev: { data: any }) => void) | null = null;
	#previewTimer: ReturnType<typeof setInterval> | undefined;

	constructor(private server: MockServer) {
		setTimeout(() => {
			this.readyState = 1;
			this.onopen?.({});
			server.onSocketOpen(this);
		}, 80);
	}
	deliver(data: string | ArrayBuffer) {
		if (this.readyState === 1) this.onmessage?.({ data });
	}
	send(data: string) {
		const msg = JSON.parse(data);
		if (msg.type === 'subscribePreview') {
			clearInterval(this.#previewTimer);
			const fps = Math.max(1, Math.min(30, msg.fps ?? 20));
			this.#previewTimer = setInterval(() => this.deliver(this.server.renderFrame()), 1000 / fps);
		} else if (msg.type === 'unsubscribePreview') clearInterval(this.#previewTimer);
	}
	close() {
		clearInterval(this.#previewTimer);
		this.readyState = 3;
		this.server.sockets.delete(this);
		this.onclose?.({});
	}
}

export type { Playlist };

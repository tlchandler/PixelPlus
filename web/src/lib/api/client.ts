// Typed HTTP client for pixelplusd (/api/v1). All requests go through a swappable
// `FetchLike` transport so the in-browser mock backend can serve them in demo mode.
import type * as T from './types';

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;
export type Uploader = (
	path: string,
	form: FormData,
	onProgress?: (p: number) => void,
	method?: string
) => Promise<unknown>;

export const API_BASE = '/api/v1';
/** Sent on every API request. pixelplusd refuses state-changing requests without it
 *  (a cross-site page can't add custom headers without a CORS preflight). */
export const REQUEST_HEADER = 'X-PixelPlus-Request';

export class ApiError extends Error {
	status: number;
	code: string;
	constructor(status: number, code: string, message: string) {
		super(message);
		this.status = status;
		this.code = code;
	}
}

let transport: FetchLike = (input, init) => fetch(input, init);
let uploader: Uploader = xhrUpload;
let onUnauthorized: (() => void) | null = null;

export function setTransport(f: FetchLike, u?: Uploader) {
	transport = f;
	if (u) uploader = u;
}
export function setUnauthorizedHandler(fn: () => void) {
	onUnauthorized = fn;
}

async function parseError(res: Response): Promise<ApiError> {
	let code = 'http_' + res.status;
	let message = res.statusText || 'Request failed';
	try {
		const body = (await res.json()) as T.ApiErrorBody;
		if (body?.error) {
			code = body.error.code;
			message = body.error.message;
		}
	} catch {
		/* not json */
	}
	return new ApiError(res.status, code, message);
}

export async function request<R = unknown>(
	method: string,
	path: string,
	body?: unknown,
	opts: { raw?: 'text' | 'blob'; signal?: AbortSignal } = {}
): Promise<R> {
	const init: RequestInit = {
		method,
		credentials: 'same-origin',
		signal: opts.signal,
		headers: { [REQUEST_HEADER]: '1' }
	};
	if (body !== undefined) {
		if (body instanceof FormData) init.body = body;
		else {
			init.body = JSON.stringify(body);
			(init.headers as Record<string, string>)['Content-Type'] = 'application/json';
		}
	}
	const res = await transport(API_BASE + path, init);
	if (!res.ok) {
		const err = await parseError(res);
		if (res.status === 401 && !path.startsWith('/auth') && !path.startsWith('/public')) onUnauthorized?.();
		throw err;
	}
	if (opts.raw === 'text') return (await res.text()) as R;
	if (opts.raw === 'blob') return (await res.blob()) as R;
	if (res.status === 204) return undefined as R;
	const text = await res.text();
	return (text ? JSON.parse(text) : undefined) as R;
}

function xhrUpload(path: string, form: FormData, onProgress?: (p: number) => void, method = 'POST') {
	return new Promise<unknown>((resolve, reject) => {
		const xhr = new XMLHttpRequest();
		xhr.open(method, API_BASE + path);
		xhr.withCredentials = true;
		xhr.setRequestHeader(REQUEST_HEADER, '1');
		xhr.upload.onprogress = (e) => e.lengthComputable && onProgress?.(e.loaded / e.total);
		xhr.onload = () => {
			let body: any;
			try {
				body = xhr.responseText ? JSON.parse(xhr.responseText) : undefined;
			} catch {
				body = xhr.responseText;
			}
			if (xhr.status >= 200 && xhr.status < 300) resolve(body);
			else
				reject(
					new ApiError(
						xhr.status,
						body?.error?.code ?? 'upload_failed',
						body?.error?.message ?? 'Upload failed'
					)
				);
		};
		xhr.onerror = () => reject(new ApiError(0, 'network', 'Network error during upload'));
		xhr.send(form);
	});
}

export function upload<R = unknown>(
	path: string,
	form: FormData,
	onProgress?: (p: number) => void
): Promise<R> {
	return uploader(path, form, onProgress) as Promise<R>;
}

const get = <R>(p: string) => request<R>('GET', p);
const post = <R>(p: string, b?: unknown) => request<R>('POST', p, b ?? {});
const put = <R>(p: string, b?: unknown) => request<R>('PUT', p, b ?? {});
const del = <R>(p: string) => request<R>('DELETE', p);

function crud<E extends { id: string }>(base: string) {
	return {
		list: () => get<E[]>(base),
		get: (id: string) => get<E>(`${base}/${id}`),
		/** `id` may be supplied (used by undo to restore a deleted entity with the same id). */
		create: (e: Partial<E>) => post<E>(base, e),
		update: (id: string, e: Partial<E>) => put<E>(`${base}/${id}`, e),
		remove: (id: string) => del<void>(`${base}/${id}`)
	};
}

export const api = {
	// ---- system
	system: () => get<T.SystemInfo>('/system'),
	setup: (req: T.SetupRequest) => post<T.SystemInfo>('/system/setup', req),
	reboot: () => post('/system/reboot'),
	shutdown: () => post('/system/shutdown'),
	restartService: () => post('/system/restart-service'),
	logs: (lines = 500) => request<string>('GET', `/system/logs?lines=${lines}`, undefined, { raw: 'text' }),
	network: () => get<T.NetworkConfig>('/system/network'),
	saveNetwork: (n: T.NetworkConfig) => put<T.NetworkConfig>('/system/network', n),
	scanWifi: () => get<T.WifiNetwork[]>('/system/network/scan'),
	sensors: () => get<T.Sensor[]>('/system/sensors'),
	sensorHistory: (minutes = 60) => get<T.SensorHistory>(`/system/sensors/history?minutes=${minutes}`),
	writeEeprom: (board: T.BoardKind, rev: string) => post('/system/eeprom', { board, rev }),
	checkUpdate: () => get<T.UpdateInfo>('/system/update'),
	applyUpdate: () => post<{ ok: boolean; message: string; job?: T.HelperStatus | null }>('/system/update'),
	helpers: () => get<T.HelperStatus[]>('/system/helpers'),
	ssh: () => get<T.SshState>('/system/ssh'),
	setSsh: (enabled: boolean) => put<{ ok: boolean; job: T.HelperStatus }>('/system/ssh', { enabled }),
	reapply: () => post<{ ok: boolean; job: T.HelperStatus }>('/system/reapply'),
	outputGeometry: () => get<T.OutputGeometry>('/system/output-geometry'),
	applyOutputGeometry: (reboot = true) =>
		post<{ ok: boolean; job: T.HelperStatus; geometry: T.OutputGeometry }>('/system/output-geometry/apply', {
			reboot
		}),
	audioDevices: () => get<{ id: string; name: string }[]>('/system/audio/devices'),

	// ---- auth
	login: (password: string) => post('/auth/login', { password }),
	logout: () => post('/auth/logout'),
	setPassword: (current: string | undefined, password: string | null) =>
		put('/auth/password', { current, password }),

	// ---- show
	show: () => get<T.Show>('/show'),
	renameShow: (name: string) => put<T.Show>('/show/name', { name }),
	saveSettings: (s: Partial<T.ShowSettings> | Record<string, unknown>) =>
		put<T.ShowSettings>('/show/settings', s),

	// ---- nodes
	nodes: crud<T.Node>('/nodes'),
	discovered: () => get<T.DiscoveredNode[]>('/nodes/discovered'),
	adopt: (id: string, name?: string, force?: boolean) => post<T.Node>('/nodes/adopt', { id, name, force }),
	/** Let another show leader adopt this controller for the next 15 minutes. */
	joinShow: (leaderUrl?: string) => post<T.JoinWindow>('/system/join-show', { leaderUrl }),
	joinStatus: () => get<T.JoinWindow>('/system/join-show'),
	cancelJoin: () => del<T.JoinWindow>('/system/join-show'),
	identifyNode: (id: string) => post(`/nodes/${id}/identify`),
	saveOutput: (nodeId: string, index: number, o: Partial<T.OutputConfig>) =>
		put<T.OutputConfig>(`/nodes/${nodeId}/outputs/${index}`, o),

	receivers: crud<T.Receiver>('/receivers'),
	props: {
		...crud<T.Prop>('/props'),
		bulk: (ops: { op: 'update' | 'delete'; id: string; patch?: Partial<T.Prop> }[]) =>
			post<T.Prop[]>('/props/bulk', { ops }),
		reorder: (ids: string[]) => post('/props/reorder', { ids })
	},
	groups: crud<T.PropGroup>('/prop-groups'),
	effects: {
		...crud<T.EffectPreset>('/effects'),
		schema: () => get<T.EffectSchema>('/effects/schema')
	},
	playlists: crud<T.Playlist>('/playlists'),
	djClips: {
		...crud<T.DjClip>('/dj-clips'),
		render: (id: string) => post<T.DjClip>(`/dj-clips/${id}/render`),
		upload: (id: string, blob: Blob, onProgress?: (p: number) => void) => {
			const f = new FormData();
			f.append('audio', blob, `${id}.wav`);
			return upload<T.DjClip>(`/dj-clips/${id}/upload`, f, onProgress);
		}
	},
	djVoices: crud<T.DjVoice>('/dj-voices'),
	savePronunciations: (list: T.Pronunciation[]) => put<T.Pronunciation[]>('/pronunciations', list),

	// ---- import
	importXlights: (rgbeffects: File, networks?: File) => {
		const f = new FormData();
		f.append('rgbeffects', rgbeffects);
		if (networks) f.append('networks', networks);
		return request<T.ImportPreview>('POST', '/import/xlights', f);
	},
	applyImport: (preview: T.ImportPreview, controllerMap: Record<string, string>) =>
		post<T.Show>('/import/xlights/apply', { preview, controllerMap }),

	// ---- content
	sequences: {
		...crud<T.Sequence>('/sequences'),
		upload: (fseq: File, audio?: File, onProgress?: (p: number) => void) => {
			const f = new FormData();
			f.append('fseq', fseq);
			if (audio) f.append('audio', audio);
			return upload<T.Sequence>('/sequences', f, onProgress);
		},
		thumbnailUrl: (id: string) => `${API_BASE}/sequences/${id}/thumbnail`
	},
	media: {
		...crud<T.Media>('/media'),
		upload: (file: File, kind: T.MediaKind = 'song', onProgress?: (p: number) => void) => {
			const f = new FormData();
			f.append('file', file);
			f.append('kind', kind);
			return upload<T.Media>('/media', f, onProgress);
		},
		fileUrl: (id: string) => `${API_BASE}/media/${id}/file`,
		peaks: (id: string, n = 160) => get<number[]>(`/media/${id}/peaks?n=${n}`)
	},

	// ---- schedule
	schedule: () => get<T.Schedule>('/schedule'),
	saveSchedule: (s: T.Schedule) => put<T.Schedule>('/schedule', s),
	schedulePreview: (days = 14) => get<T.ScheduleOccurrence[]>(`/schedule/preview?days=${days}`),

	// ---- player
	player: () => get<T.PlayerStatus>('/player'),
	play: (what: { playlistId?: string; sequenceId?: string; djClipId?: string } = {}) =>
		post('/player/play', what),
	stop: (fade = false) => post('/player/stop', { fade }),
	pause: () => post('/player/pause'),
	resume: () => post('/player/resume'),
	next: () => post('/player/next'),
	previous: () => post('/player/previous'),
	seek: (posMs: number) => post('/player/seek', { posMs }),
	setVolume: (volume: number) => put('/player/volume', { volume }),
	setBrightness: (brightness: number) => put('/player/brightness', { brightness }),
	blackout: (enabled: boolean) => post('/player/blackout', { enabled }),
	applyEffect: (effect: T.EffectPreset | null) => post('/player/effect', { effect }),
	/** "Sync lights to sound": click + white flash every second on every controller. */
	calibration: (on: boolean) => post('/player/calibration', { on }),

	// ---- tests & tools
	testStart: (req: T.TestRequest) => post('/test/start', req),
	testStop: () => post('/test/stop'),
	faultStart: (propId: string) => post<T.FaultStep>('/faultfinder/start', { propId }),
	faultAnswer: (session: string, lit: boolean) =>
		post<T.FaultStep>(`/faultfinder/${session}/answer`, { lit }),
	faultStop: () => post('/faultfinder/stop'),
	power: (sequenceId?: string) =>
		get<T.PowerEstimate>(`/power/estimate${sequenceId ? `?sequenceId=${sequenceId}` : ''}`),
	health: () => get<T.HealthReport>('/health'),
	runHealth: () => post<T.HealthReport>('/health/run'),

	// ---- snapshots
	snapshots: () => get<T.Snapshot[]>('/snapshots'),
	createSnapshot: (label: string) => post<T.Snapshot>('/snapshots', { label }),
	restoreSnapshot: (id: string) => post(`/snapshots/${id}/restore`),
	deleteSnapshot: (id: string) => del(`/snapshots/${id}`),
	snapshotDownloadUrl: (id: string) => `${API_BASE}/snapshots/${id}/download`,
	importSnapshot: (file: File, onProgress?: (p: number) => void) => {
		const f = new FormData();
		f.append('file', file);
		return upload<T.Snapshot>('/snapshots/import', f, onProgress);
	},

	// ---- tts
	ttsStatus: () => get<T.TtsStatus>('/tts/status'),
	ttsRender: (lines: T.DjLine[], speed = 1) =>
		request<Blob>('POST', '/tts/render', { lines, speed }, { raw: 'blob' }),

	// ---- requests
	publicRequests: () => get<T.PublicRequests>('/public/requests'),
	submitRequest: (sequenceId: string, name?: string) => post('/public/requests', { sequenceId, name }),
	requests: () => get<T.SongRequest[]>('/requests'),
	removeRequest: (id: string) => del(`/requests/${id}`),

	// ---- alerts / integrations
	testAlert: (channel: 'email' | 'ntfy') =>
		post<{ ok: boolean; message: string }>('/alerts/test', { channel }),
	testMqtt: () => post<{ ok: boolean; message: string }>('/mqtt/test'),

	// ---- games
	games: {
		status: () => get<T.GamesStatus>('/games/status'),
		invite: () => post('/games/invite'),
		stop: () => post('/games/stop'),
		testPattern: (propId: string) => post('/games/test-pattern', { propId }),
		roms: () => get<{ name: string; sizeBytes: number }[]>('/games/roms'),
		uploadRom: (file: File, onProgress?: (p: number) => void) => {
			const f = new FormData();
			f.append('rom', file);
			return upload('/games/roms', f, onProgress);
		},
		removeRom: (name: string) => del(`/games/roms/${encodeURIComponent(name)}`)
	}
};

export type Api = typeof api;

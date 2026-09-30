import { api, ApiError, setUnauthorizedHandler } from '$lib/api/client';
import { initBackend } from '$lib/api/mode';
import { openSocket, parsePreviewFrame, type SocketLike } from '$lib/api/socket';
import type {
	HelperStatus,
	LogLine,
	NodeStatus,
	PlayerStatus,
	Sensor,
	Show,
	SystemInfo
} from '$lib/api/types';
import { toasts } from './toasts.svelte';

type PreviewCb = (rgb: Uint8Array, frameNo: number) => void;

class AppState {
	ready = $state(false);
	booting = $state(true);
	mock = $state(false);
	mockAuto = $state(false);
	fatal = $state<string | null>(null);
	needsLogin = $state(false);
	system = $state<SystemInfo | null>(null);
	/** Full show; replaced wholesale on every change (use updateShow for optimistic edits). */
	show = $state.raw<Show | null>(null);
	connection = $state<'connecting' | 'open' | 'closed'>('connecting');
	status = $state<PlayerStatus | null>(null);
	nodes = $state<NodeStatus[]>([]);
	sensors = $state<Sensor[]>([]);
	logs = $state<LogLine[]>([]);
	/** Root helper jobs by verb (boot settings, updates, SSH…), from `helper` messages. */
	helpers = $state<Record<string, HelperStatus>>({});

	#ws: SocketLike | null = null;
	#retry = 0;
	#retryTimer: ReturnType<typeof setTimeout> | undefined;
	#previewSubs = new Set<PreviewCb>();
	#previewFps = 20;
	#reloading: Promise<void> | null = null;
	#pendingVersion = 0;
	#started = false;

	async boot() {
		if (this.#started) return;
		this.#started = true;
		setUnauthorizedHandler(() => (this.needsLogin = true));
		try {
			const mode = await initBackend();
			this.mock = mode.mock;
			this.mockAuto = mode.auto;
			await this.loadSystem();
			if (!this.system?.needsSetup && !this.needsLogin && this.system?.role !== 'follower')
				await this.reloadShow();
			this.connect();
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) this.needsLogin = true;
			else this.fatal = e instanceof Error ? e.message : String(e);
		} finally {
			this.booting = false;
			this.ready = true;
		}
	}

	async loadSystem() {
		try {
			this.system = await api.system();
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) this.needsLogin = true;
			else throw e;
		}
	}

	async afterLogin() {
		this.needsLogin = false;
		await this.loadSystem();
		await this.reloadShow();
		this.connect();
	}

	reloadShow(): Promise<void> {
		if (this.#reloading) return this.#reloading;
		this.#reloading = (async () => {
			try {
				// A change announced while this fetch was in flight may not be in its
				// answer: fetch again (bounded) until we have at least that version.
				for (let i = 0; i < 3; i++) {
					this.show = await api.show();
					if (this.show.version >= this.#pendingVersion) break;
				}
			} catch (e) {
				if (!(e instanceof ApiError && e.status === 401)) throw e;
			} finally {
				this.#reloading = null;
			}
		})();
		return this.#reloading;
	}

	/** Optimistically edit the local show copy (UI updates instantly; server echo replaces it). */
	updateShow(fn: (s: Show) => void) {
		if (!this.show) return;
		const copy = structuredClone($state.snapshot(this.show)) as Show;
		fn(copy);
		this.show = copy;
	}

	/** Run a mutation; on success refresh the show, on failure toast and refresh. */
	async mutate<R>(
		fn: () => Promise<R>,
		opts: { success?: string; error?: string } = {}
	): Promise<R | undefined> {
		try {
			const r = await fn();
			if (opts.success) toasts.success(opts.success);
			await this.reloadShow();
			return r;
		} catch (e) {
			toasts.error(opts.error ?? 'Something went wrong', e instanceof Error ? e.message : String(e));
			await this.reloadShow().catch(() => {});
			return undefined;
		}
	}

	// ------------------------------------------------------------- websocket
	connect() {
		if (this.#ws) return;
		this.connection = 'connecting';
		let ws: SocketLike;
		try {
			ws = openSocket();
		} catch {
			this.#scheduleReconnect();
			return;
		}
		ws.binaryType = 'arraybuffer';
		this.#ws = ws;
		ws.onopen = () => {
			this.connection = 'open';
			this.#retry = 0;
			if (this.#previewSubs.size)
				ws.send(JSON.stringify({ type: 'subscribePreview', fps: this.#previewFps }));
			// We may have missed show changes while disconnected.
			if (this.show) this.reloadShow().catch(() => {});
		};
		ws.onmessage = (ev) => this.#onMessage(ev.data);
		ws.onclose = () => {
			if (this.#ws !== ws) return;
			this.#ws = null;
			this.connection = 'closed';
			this.#scheduleReconnect();
		};
		ws.onerror = () => {
			/* close follows */
		};
	}

	#scheduleReconnect() {
		clearTimeout(this.#retryTimer);
		const delay = Math.min(10000, 500 * 2 ** this.#retry++);
		this.#retryTimer = setTimeout(() => this.connect(), delay);
	}

	#onMessage(data: unknown) {
		if (data instanceof ArrayBuffer) {
			const f = parsePreviewFrame(data);
			if (f) for (const cb of this.#previewSubs) cb(f.rgb, f.frameNo);
			return;
		}
		let msg: { type: string; data: any };
		try {
			msg = JSON.parse(String(data));
		} catch {
			return;
		}
		switch (msg.type) {
			case 'status':
				this.status = msg.data;
				break;
			case 'show':
				if (
					this.show &&
					msg.data?.version !== this.show.version &&
					msg.data?.version !== this.#pendingVersion
				) {
					this.#pendingVersion = msg.data.version;
					this.reloadShow().catch(() => {});
				}
				break;
			case 'nodes':
				this.nodes = msg.data;
				break;
			case 'sensors':
				this.sensors = msg.data;
				break;
			case 'log':
				this.logs = [msg.data, ...this.logs].slice(0, 200);
				break;
			case 'toast':
				toasts.push({ kind: msg.data.kind, message: msg.data.message });
				break;
			case 'helper':
				if (msg.data?.verb) this.helpers = { ...this.helpers, [msg.data.verb]: msg.data };
				break;
			case 'system':
				// e.g. settings from pixelplus.txt were applied (role, board, password)
				this.loadSystem().catch(() => {});
				break;
		}
	}

	/** Receive live preview frames (RGB for every prop in show.props order). */
	subscribePreview(cb: PreviewCb, fps = 20): () => void {
		this.#previewSubs.add(cb);
		if (this.#previewSubs.size === 1 || fps > this.#previewFps) {
			this.#previewFps = Math.max(fps, this.#previewSubs.size === 1 ? fps : this.#previewFps);
			if (this.#ws?.readyState === 1)
				this.#ws.send(JSON.stringify({ type: 'subscribePreview', fps: this.#previewFps }));
		}
		return () => {
			this.#previewSubs.delete(cb);
			if (!this.#previewSubs.size && this.#ws?.readyState === 1)
				this.#ws.send(JSON.stringify({ type: 'unsubscribePreview' }));
		};
	}

	/** Byte offset of each prop in a preview frame. */
	previewOffsets(): Map<string, number> {
		const m = new Map<string, number>();
		let o = 0;
		for (const p of this.show?.props ?? []) {
			m.set(p.id, o);
			o += p.pixelCount * 3;
		}
		return m;
	}
}

export const app = new AppState();

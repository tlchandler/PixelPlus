// Plays a sequence preview on this device only (F3): downloads the PPPV preview file
// block by block and draws it in time with the song. The song's <audio> element is the
// master clock; without audio (light-only sequences, demo mode) a local clock runs.
// Never sends anything to the lights.
import { api, ApiError } from '$lib/api/client';
import { app } from '$lib/stores/app.svelte';
import { isMock } from '$lib/api/mode';
import { library } from '$lib/library/api';
import { PppvReader, frameIndex, type PppvHeader } from './pppv';
import type { FrameDrawer, FrameSource, PropSlot } from './source';

export type PreviewStatus = 'idle' | 'preparing' | 'ready' | 'error';

export class PreviewPlayer {
	status = $state<PreviewStatus>('idle');
	/** Server-side preparation progress, 0..100. */
	pct = $state(0);
	playing = $state(false);
	posMs = $state(0);
	durationMs = $state(0);
	rate = $state(1);
	/** Waiting for a block to download. */
	buffering = $state(false);
	/** The song plays along (false: light-only or audio unavailable). */
	withAudio = $state(false);
	error = $state<string | null>(null);
	seqId = $state<string | null>(null);

	#reader: PppvReader | null = null;
	#audio: HTMLAudioElement | null = null;
	#drawers = new Set<FrameDrawer>();
	#raf = 0;
	#clockStart = 0;
	#clockAt = 0;
	#gen = 0;
	#unsubJob: (() => void) | null = null;
	#poll: ReturnType<typeof setTimeout> | undefined;
	#last: Uint8Array | null = null;
	#lastFrame = -1;

	/** Frame source for LayoutCanvas. */
	readonly source: FrameSource = {
		kind: 'preview',
		subscribe: (d) => {
			this.#drawers.add(d);
			if (this.#last && this.#reader) d(this.#last, this.#reader.slots);
			return () => this.#drawers.delete(d);
		}
	};

	get header(): PppvHeader | null {
		return this.#reader?.header ?? null;
	}

	/** Load the preview of a sequence (and its song). Resolves when ready or failed. */
	async load(seqId: string, mediaId?: string): Promise<void> {
		this.unload();
		const gen = ++this.#gen;
		this.seqId = seqId;
		this.status = 'preparing';
		this.pct = 0;
		this.error = null;
		let header: PppvHeader;
		try {
			header = await this.#waitForHeader(seqId, gen);
		} catch (e) {
			if (gen !== this.#gen) return;
			this.status = 'error';
			this.error = e instanceof Error ? e.message : String(e);
			return;
		}
		if (gen !== this.#gen) return;
		const reader = new PppvReader(header, (n) => library.previewBlock(seqId, n));
		reader.onblock = (_n, err) => {
			if (err && gen === this.#gen) this.error = 'Part of the preview didn’t download. Check the connection.';
			if (gen === this.#gen && !this.playing) this.#render(true);
		};
		this.#reader = reader;
		this.durationMs = reader.durationMs;
		this.posMs = 0;
		this.#setupAudio(mediaId, gen);
		await reader.prefetch(0);
		if (gen !== this.#gen) return;
		this.status = 'ready';
		this.#render(true);
	}

	async #waitForHeader(seqId: string, gen: number): Promise<PppvHeader> {
		for (;;) {
			const r = await library.preview(seqId);
			if (r.ready) return r.header;
			this.pct = Math.max(this.pct, r.pct);
			// Progress arrives as WS `job` messages; poll too in case the socket is down.
			await new Promise<void>((resolve, reject) => {
				this.#unsubJob?.();
				this.#unsubJob = app.onMessage('job', (j) => {
					if (j.id !== r.jobId) return;
					this.pct = Math.max(this.pct, j.pct);
					if (j.state === 'done') resolve();
					if (j.state === 'failed') reject(new Error(j.result?.message ?? 'The preview couldn’t be made.'));
				});
				clearTimeout(this.#poll);
				this.#poll = setTimeout(resolve, 1500);
			});
			this.#unsubJob?.();
			this.#unsubJob = null;
			clearTimeout(this.#poll);
			if (gen !== this.#gen) throw new Error('cancelled');
		}
	}

	#setupAudio(mediaId: string | undefined, gen: number) {
		this.withAudio = false;
		// Demo mode has no real audio files: the local clock runs the preview.
		if (!mediaId || typeof Audio === 'undefined' || isMock()) return;
		const a = new Audio();
		a.preload = 'auto';
		a.src = api.media.fileUrl(mediaId);
		a.onerror = () => {
			if (gen !== this.#gen || this.#audio !== a) return;
			// Fall back to the local clock at the same position.
			const pos = this.posMs;
			this.#audio = null;
			this.withAudio = false;
			this.#startClock(pos);
		};
		a.onended = () => {
			if (gen === this.#gen) this.#ended();
		};
		this.#audio = a;
		this.withAudio = true;
	}

	#startClock(fromMs: number) {
		this.#clockAt = fromMs;
		this.#clockStart = performance.now();
	}

	/** Current position (ms): the audio clock when there is one. */
	#now(): number {
		if (this.#audio) return this.#audio.currentTime * 1000;
		if (!this.playing) return this.posMs;
		return this.#clockAt + (performance.now() - this.#clockStart) * this.rate;
	}

	async play() {
		if (!this.#reader || this.status !== 'ready') return;
		if (this.posMs >= this.durationMs - 50) await this.seek(0);
		this.playing = true;
		if (this.#audio) {
			this.#audio.playbackRate = this.rate;
			try {
				await this.#audio.play();
			} catch {
				// Autoplay refused or no audio: keep going silently.
				this.#audio = null;
				this.withAudio = false;
			}
		}
		this.#startClock(this.posMs);
		cancelAnimationFrame(this.#raf);
		this.#raf = requestAnimationFrame(this.#tick);
	}

	pause() {
		this.posMs = this.#now();
		this.playing = false;
		this.#audio?.pause();
		cancelAnimationFrame(this.#raf);
	}

	toggle() {
		if (this.playing) this.pause();
		else void this.play();
	}

	async seek(ms: number) {
		const d = this.durationMs;
		const to = Math.max(0, Math.min(d, ms));
		this.posMs = to;
		if (this.#audio) this.#audio.currentTime = to / 1000;
		this.#startClock(to);
		this.#render(true);
		if (this.#reader) {
			this.buffering = true;
			await this.#reader.prefetch(to);
			this.buffering = false;
			this.#render(true);
		}
	}

	setRate(r: number) {
		this.posMs = this.#now();
		this.rate = r;
		if (this.#audio) this.#audio.playbackRate = r;
		this.#startClock(this.posMs);
	}

	#tick = () => {
		if (!this.playing) return;
		this.posMs = this.#now();
		if (this.posMs >= this.durationMs) {
			this.#ended();
			return;
		}
		this.#render(false);
		this.#raf = requestAnimationFrame(this.#tick);
	};

	#ended() {
		this.playing = false;
		this.posMs = this.durationMs;
		cancelAnimationFrame(this.#raf);
		this.#audio?.pause();
		this.#render(true);
	}

	#render(force: boolean) {
		const r = this.#reader;
		if (!r) return;
		const f = frameIndex(r.header, this.posMs);
		if (!force && f === this.#lastFrame) return;
		const rgb = r.frameAt(this.posMs);
		this.buffering = !rgb && this.playing;
		if (!rgb) return;
		this.#lastFrame = f;
		this.#last = rgb;
		for (const d of this.#drawers) d(rgb, r.slots as Map<string, PropSlot>);
	}

	/** Stop and forget the current preview. */
	unload() {
		this.#gen++;
		this.pause();
		this.#unsubJob?.();
		this.#unsubJob = null;
		clearTimeout(this.#poll);
		if (this.#audio) {
			this.#audio.onerror = null;
			this.#audio.src = '';
		}
		this.#audio = null;
		this.#reader = null;
		this.#last = null;
		this.#lastFrame = -1;
		this.status = 'idle';
		this.seqId = null;
		this.posMs = 0;
		this.durationMs = 0;
		this.buffering = false;
		this.error = null;
	}
}

/** Friendly text for preview errors. */
export function previewError(e: unknown): string {
	if (e instanceof ApiError && e.status === 404) return 'That sequence is gone.';
	return e instanceof Error ? e.message : String(e);
}

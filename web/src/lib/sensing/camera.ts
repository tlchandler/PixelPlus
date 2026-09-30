/**
 * Phone camera (and microphone) access for measuring pages: friendly errors, frame callbacks
 * with capture timestamps, and an exposure lock so auto-exposure doesn't "pump" on flashes.
 * Shared with camera mapping (F6/F7).
 */

/** Microphone with every "voice" processing step off (they smear and delay clicks). */
export const RAW_AUDIO: MediaTrackConstraints = {
	echoCancellation: false,
	noiseSuppression: false,
	autoGainControl: false,
	channelCount: 1
};

export type MediaProblem = 'insecure' | 'unsupported' | 'denied' | 'notFound' | 'inUse' | 'other';

export class MediaAccessError extends Error {
	constructor(
		readonly kind: MediaProblem,
		message: string
	) {
		super(message);
	}
}

/** What went wrong, in words for the page. */
export function describeMediaError(e: unknown, what = 'camera and microphone'): MediaAccessError {
	if (e instanceof MediaAccessError) return e;
	const name = (e as { name?: string })?.name ?? '';
	if (name === 'NotAllowedError' || name === 'SecurityError')
		return new MediaAccessError(
			'denied',
			`Your phone didn't allow the ${what}. Tap the lock or settings icon next to the address, allow them, then try again.`
		);
	if (name === 'NotFoundError' || name === 'OverconstrainedError')
		return new MediaAccessError(
			'notFound',
			`This device doesn't seem to have a ${what.replace(' and ', ' or ')}.`
		);
	if (name === 'NotReadableError' || name === 'AbortError')
		return new MediaAccessError('inUse', `Another app is using the ${what}. Close it and try again.`);
	return new MediaAccessError('other', `Couldn't start the ${what}: ${(e as Error)?.message ?? e}`);
}

export interface MediaOptions {
	/** 'environment' = the back camera (default). */
	facingMode?: 'environment' | 'user';
	width?: number;
	height?: number;
	frameRate?: number;
	/** Also open the microphone (raw, see RAW_AUDIO). */
	audio?: boolean;
	/** No camera (microphone only). */
	video?: boolean;
}

/** Ask for the camera (and microphone). Throws `MediaAccessError`. */
export async function openMedia(o: MediaOptions = {}): Promise<MediaStream> {
	if (typeof window !== 'undefined' && !window.isSecureContext)
		throw new MediaAccessError('insecure', 'The camera and microphone need a secure (https) page.');
	if (!navigator.mediaDevices?.getUserMedia)
		throw new MediaAccessError('unsupported', "This browser can't use the camera here. Try Chrome.");
	const video: MediaTrackConstraints | false =
		o.video === false
			? false
			: {
					facingMode: { ideal: o.facingMode ?? 'environment' },
					width: { ideal: o.width ?? 640 },
					height: { ideal: o.height ?? 480 },
					frameRate: { ideal: o.frameRate ?? 30 }
				};
	try {
		return await navigator.mediaDevices.getUserMedia({ video, audio: o.audio ? RAW_AUDIO : false });
	} catch (e) {
		throw describeMediaError(e, o.audio ? (video ? 'camera and microphone' : 'microphone') : 'camera');
	}
}

export interface CameraFrame {
	/** Capture time on the phone clock (performance.now() ms). */
	t: number;
	/** `t` is the sensor's capture time (else estimated from the display time). */
	hasCaptureTime: boolean;
	index: number;
	width: number;
	height: number;
}

export interface ExposureLock {
	locked: boolean;
	/** Exposure time actually set (ms), when locked. */
	exposureMs?: number;
	reason?: string;
}

type Caps = MediaTrackCapabilities & {
	exposureMode?: string[];
	exposureTime?: { min: number; max: number; step?: number };
};

export class CameraCapture {
	readonly track: MediaStreamTrack;
	readonly video: HTMLVideoElement;
	private cbId = 0;
	private listeners = new Set<(f: CameraFrame) => void>();
	private index = 0;
	private lastT = 0;
	private intervals: number[] = [];
	private running = false;

	constructor(stream: MediaStream, video?: HTMLVideoElement) {
		const track = stream.getVideoTracks()[0];
		if (!track) throw new MediaAccessError('notFound', 'No camera in this stream.');
		this.track = track;
		this.video = video ?? document.createElement('video');
		this.video.muted = true;
		this.video.playsInline = true;
		this.video.setAttribute('playsinline', '');
		this.video.srcObject = new MediaStream([track]);
	}

	async start(): Promise<void> {
		await this.video.play().catch(() => {});
		this.running = true;
		const hasRvfc = 'requestVideoFrameCallback' in HTMLVideoElement.prototype;
		if (hasRvfc) {
			const loop: VideoFrameRequestCallback = (_now, meta) => {
				if (!this.running) return;
				this.frame(meta);
				this.cbId = this.video.requestVideoFrameCallback(loop);
			};
			this.cbId = this.video.requestVideoFrameCallback(loop);
		} else {
			// Old browsers: poll at display rate (timestamps are rough).
			const poll = () => {
				if (!this.running) return;
				this.frame(null);
				this.cbId = requestAnimationFrame(poll);
			};
			this.cbId = requestAnimationFrame(poll);
		}
	}

	private frame(meta: VideoFrameCallbackMetadata | null) {
		const period = this.frameIntervalMs();
		let t: number;
		let has = false;
		if (meta?.captureTime) {
			t = meta.captureTime;
			has = true;
		} else if (meta) t = meta.expectedDisplayTime - period;
		else t = performance.now() - period;
		if (this.lastT) {
			const d = t - this.lastT;
			if (d > 0 && d < 250) {
				this.intervals.push(d);
				if (this.intervals.length > 60) this.intervals.shift();
			}
		}
		this.lastT = t;
		const f: CameraFrame = {
			t,
			hasCaptureTime: has,
			index: this.index++,
			width: meta?.width ?? this.video.videoWidth,
			height: meta?.height ?? this.video.videoHeight
		};
		for (const l of this.listeners) l(f);
	}

	/** Called for every camera frame (in display order). Returns an unsubscribe function. */
	onFrame(cb: (f: CameraFrame) => void): () => void {
		this.listeners.add(cb);
		return () => this.listeners.delete(cb);
	}

	/** Median frame interval so far (ms). */
	frameIntervalMs(): number {
		if (!this.intervals.length) return 1000 / (this.track.getSettings().frameRate || 30);
		const s = [...this.intervals].sort((a, b) => a - b);
		return s[s.length >> 1];
	}

	get fps(): number {
		return 1000 / this.frameIntervalMs();
	}

	/**
	 * Fix the exposure near one frame time (so a flash that starts mid-frame lights part of it,
	 * which the timing uses) and stop auto-exposure reacting to the flashes. Falls back to
	 * automatic exposure when the phone doesn't allow it (the detection copes).
	 */
	async lockExposure(): Promise<ExposureLock> {
		const caps = (this.track.getCapabilities?.() ?? {}) as Caps;
		if (!caps.exposureMode?.includes('manual') || !caps.exposureTime)
			return { locked: false, reason: "This phone doesn't let web pages fix the exposure." };
		// Units: 100 µs.
		const want = (this.frameIntervalMs() * 0.95) / 0.1;
		const t = Math.min(caps.exposureTime.max, Math.max(caps.exposureTime.min, want));
		try {
			await this.track.applyConstraints({
				advanced: [{ exposureMode: 'manual', exposureTime: t } as MediaTrackConstraintSet]
			});
			return { locked: true, exposureMs: t * 0.1 };
		} catch (e) {
			return { locked: false, reason: (e as Error).message };
		}
	}

	async unlockExposure(): Promise<void> {
		const caps = (this.track.getCapabilities?.() ?? {}) as Caps;
		if (!caps.exposureMode?.includes('continuous')) return;
		await this.track
			.applyConstraints({ advanced: [{ exposureMode: 'continuous' } as MediaTrackConstraintSet] })
			.catch(() => {});
	}

	stop(): void {
		this.running = false;
		if ('cancelVideoFrameCallback' in this.video) this.video.cancelVideoFrameCallback(this.cbId);
		cancelAnimationFrame(this.cbId);
		this.listeners.clear();
		this.video.srcObject = null;
	}
}

/** Stop every track of a stream (camera light off). */
export function stopStream(stream: MediaStream | null | undefined): void {
	stream?.getTracks().forEach((t) => t.stop());
}

/**
 * A running camera + microphone session for "Measure with my phone": captures, detects clicks
 * and flashes continuously (for the live "seeing / hearing" meters), and hands out what was
 * detected inside a measurement window, all on the phone clock (performance.now() ms).
 */
import { ChirpDetector } from './audio-onset';
import { CameraCapture, openMedia, stopStream } from './camera';
import { FrameAnalyzer } from './frames';
import { MicCapture, type MicLatency } from './mic';
import { template, type CalPlan } from './schedule';
import { detectFlashes, type FrameMetric, type FrameSample } from './video-onset';

export interface FrameRecord extends FrameMetric {
	t: number;
}

export interface LiveReadings {
	fps: number;
	/** Picture brightness 0..1 (linear). */
	brightness: number;
	/** Flashes / clicks in the last few seconds. */
	flashesSeen: number;
	clicksHeard: number;
	seeingFlashes: boolean;
	hearingClicks: boolean;
	/** Loudness of the recent clicks above the background (robust σ). */
	clickSnr: number;
	exposureLocked: boolean;
	/** The camera reports real capture times (best timing). */
	captureTimes: boolean;
	micLatency: MicLatency;
}

const RECENT_MS = 6000;
const KEEP_FRAMES_MS = 120_000;

export class SensingSession {
	readonly stream: MediaStream;
	readonly camera: CameraCapture;
	readonly mic: MicCapture;
	private analyzer = new FrameAnalyzer();
	private detector: ChirpDetector | null = null;
	private nextFrame = -1;
	private frames: FrameRecord[] = [];
	private clicks: { t: number; snr: number }[] = [];
	private raw: { chunks: Float32Array[]; firstFrame: number } | null = null;
	private offFrame: () => void;
	private offChunk: () => void;
	private plan: CalPlan | null = null;
	private flashMs = 80;
	exposureLocked = false;
	private captureTimes = false;
	private closed = false;

	private constructor(stream: MediaStream, camera: CameraCapture, mic: MicCapture) {
		this.stream = stream;
		this.camera = camera;
		this.mic = mic;
		this.offFrame = camera.onFrame((f) => {
			this.captureTimes = f.hasCaptureTime;
			void this.analyzer.analyze(camera.video).then((m) => {
				if (!m || this.closed) return;
				this.frames.push({ t: f.t, ...m });
				const cut = f.t - KEEP_FRAMES_MS;
				while (this.frames.length && this.frames[0].t < cut) this.frames.shift();
			});
		});
		this.offChunk = mic.onChunk((c) => this.onAudio(c.frame, c.samples));
	}

	/** Open the back camera and the raw microphone and start analysing. */
	static async open(video: HTMLVideoElement): Promise<SensingSession> {
		const stream = await openMedia({ audio: true, facingMode: 'environment', frameRate: 30 });
		try {
			const camera = new CameraCapture(stream, video);
			await camera.start();
			const mic = await MicCapture.open(stream);
			const s = new SensingSession(stream, camera, mic);
			// Let the frame rate settle, then fix the exposure.
			setTimeout(() => {
				if (!s.closed) void camera.lockExposure().then((r) => (s.exposureLocked = r.locked));
			}, 1200);
			return s;
		} catch (e) {
			stopStream(stream);
			throw e;
		}
	}

	/** Use this pattern's click shape from now on (the classic and v2 clicks differ). */
	setPlan(plan: CalPlan): void {
		const changed = !this.plan || (this.plan.chirp === null) !== (plan.chirp === null);
		this.plan = plan;
		this.flashMs = plan.flashMs;
		if (changed) {
			this.detector = null;
			this.nextFrame = -1;
		}
	}

	private onAudio(frame: number, samples: Float32Array) {
		if (this.closed) return;
		if (this.raw) this.raw.chunks.push(samples.slice());
		if (!this.plan) return;
		if (!this.detector || this.nextFrame < 0 || frame - this.nextFrame > this.mic.sampleRate) {
			this.detector = new ChirpDetector(this.mic.sampleRate, template(this.plan, this.mic.sampleRate), {
				startSample: frame
			});
		} else if (frame > this.nextFrame) {
			// A small glitch: keep the timing by filling the gap with silence.
			this.detector.push(new Float32Array(frame - this.nextFrame));
		}
		this.nextFrame = frame + samples.length;
		const lat = this.mic.latency();
		for (const o of this.detector.push(samples)) {
			this.clicks.push({ t: this.mic.frameToPerfMs(o.sample, lat), snr: o.snr });
			if (this.clicks.length > 2000) this.clicks.splice(0, 500);
		}
	}

	/** Frames seen from `from` to `to` (phone ms). */
	framesBetween(from: number, to: number): FrameSample[] {
		return this.frames.filter((f) => f.t >= from && f.t <= to);
	}

	/** Detected click times in [from, to]. */
	clicksBetween(from: number, to: number): number[] {
		return this.clicks.filter((c) => c.t >= from && c.t <= to).map((c) => c.t);
	}

	/** Detected flash onsets in [from, to] (uses 2.5 s before `from` for the noise floor). */
	flashesBetween(from: number, to: number): number[] {
		const frames = this.framesBetween(from - 2500, to);
		return detectFlashes(frames, { flashMs: this.flashMs })
			.map((f) => f.t)
			.filter((t) => t >= from && t <= to);
	}

	/** Readings for the meters. */
	live(): LiveReadings {
		const now = performance.now();
		const recent = this.frames.filter((f) => f.t > now - RECENT_MS - 2500);
		const flashes = detectFlashes(recent, { flashMs: this.flashMs }).filter((f) => f.t > now - RECENT_MS);
		const clicks = this.clicks.filter((c) => c.t > now - RECENT_MS);
		const last = this.frames[this.frames.length - 1];
		return {
			fps: this.camera.fps,
			brightness: last?.mean ?? 0,
			flashesSeen: flashes.length,
			clicksHeard: clicks.length,
			seeingFlashes: flashes.length >= 3,
			hearingClicks: clicks.length >= 3,
			clickSnr: clicks.length ? clicks.reduce((a, c) => a + c.snr, 0) / clicks.length : 0,
			exposureLocked: this.exposureLocked,
			captureTimes: this.captureTimes,
			micLatency: this.mic.latency()
		};
	}

	/** Start keeping raw microphone samples (clap test). */
	startRaw(): void {
		this.raw = { chunks: [], firstFrame: this.nextFrame };
	}

	/** Stop keeping raw samples; returns them with the phone time of the first one. */
	stopRaw(): { samples: Float32Array; startMs: number; sampleRate: number } | null {
		const r = this.raw;
		this.raw = null;
		if (!r || !r.chunks.length) return null;
		const n = r.chunks.reduce((a, c) => a + c.length, 0);
		const samples = new Float32Array(n);
		let o = 0;
		for (const c of r.chunks) {
			samples.set(c, o);
			o += c.length;
		}
		const first = r.firstFrame >= 0 ? r.firstFrame : this.nextFrame - n;
		return { samples, startMs: this.mic.frameToPerfMs(first), sampleRate: this.mic.sampleRate };
	}

	/** Motion series (clap test). */
	motionBetween(from: number, to: number): { t: number; motion: number }[] {
		return this.frames.filter((f) => f.t >= from && f.t <= to).map((f) => ({ t: f.t, motion: f.motion }));
	}

	close(): void {
		if (this.closed) return;
		this.closed = true;
		this.offFrame();
		this.offChunk();
		this.camera.stop();
		this.mic.close();
		this.analyzer.close();
		stopStream(this.stream);
	}
}

/**
 * Raw microphone capture with sample-accurate timing on the phone clock
 * (`performance.now()` ms).
 *
 * Samples come from an AudioWorklet (ScriptProcessor fallback) tagged with their audio-context
 * frame index. Frame → phone time uses `getOutputTimestamp()` pairs (context time ↔
 * performance time of the *output*), fitted linearly, minus the output latency (when the
 * rendered quantum is heard) and the input latency (capture → graph). Input latency comes from
 * `track.stats` (Chrome 125+) when available, else the context's base latency.
 */
import { LinearClock } from './clock';
import workletUrl from './capture.worklet.ts?worker&url';

export interface MicChunk {
	/** Audio-context frame index of `samples[0]`. */
	frame: number;
	samples: Float32Array;
}

export interface MicLatency {
	inputMs: number;
	outputMs: number;
	source: 'stats' | 'base' | 'none';
}

type StatsTrack = MediaStreamTrack & { stats?: { latency?: number; averageLatency?: number } };

export class MicCapture {
	readonly ctx: AudioContext;
	readonly track: MediaStreamTrack;
	private source: MediaStreamAudioSourceNode;
	private node: AudioNode | null = null;
	private sink: GainNode;
	private clock = new LinearClock(1000);
	private timer: ReturnType<typeof setInterval> | undefined;
	private listeners = new Set<(c: MicChunk) => void>();
	private closed = false;

	private constructor(stream: MediaStream) {
		const track = stream.getAudioTracks()[0];
		if (!track) throw new Error('No microphone in this stream');
		this.track = track;
		const rate = track.getSettings().sampleRate;
		const Ctx =
			window.AudioContext ??
			(window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
		this.ctx = new Ctx({ latencyHint: 'interactive', ...(rate ? { sampleRate: rate } : {}) });
		this.source = this.ctx.createMediaStreamSource(new MediaStream([track]));
		// Keep the graph pulling without making a sound.
		this.sink = this.ctx.createGain();
		this.sink.gain.value = 0;
		this.sink.connect(this.ctx.destination);
	}

	/** Start capturing the stream's microphone. */
	static async open(stream: MediaStream): Promise<MicCapture> {
		const m = new MicCapture(stream);
		await m.start();
		return m;
	}

	get sampleRate(): number {
		return this.ctx.sampleRate;
	}

	private async start() {
		if (this.ctx.state === 'suspended') await this.ctx.resume().catch(() => {});
		let node: AudioNode | null = null;
		if (this.ctx.audioWorklet) {
			try {
				await this.ctx.audioWorklet.addModule(workletUrl);
				const w = new AudioWorkletNode(this.ctx, 'pp-capture', {
					numberOfInputs: 1,
					numberOfOutputs: 1,
					channelCount: 1
				});
				w.port.onmessage = (e: MessageEvent<MicChunk>) => this.emit(e.data);
				node = w;
			} catch {
				node = null;
			}
		}
		if (!node) {
			// Fallback: ScriptProcessor (deprecated, but everywhere).
			const sp = this.ctx.createScriptProcessor(2048, 1, 1);
			let frame = 0;
			sp.onaudioprocess = (e) => {
				const data = new Float32Array(e.inputBuffer.getChannelData(0));
				// playbackTime is when this block's output plays: its context frame.
				const f = Math.round(e.playbackTime * this.ctx.sampleRate) || frame;
				frame = f + data.length;
				this.emit({ frame: f, samples: data });
			};
			node = sp;
		}
		this.source.connect(node);
		node.connect(this.sink);
		this.node = node;
		const sample = () => {
			if (this.closed) return;
			const ts = this.ctx.getOutputTimestamp?.();
			if (ts && ts.contextTime && ts.performanceTime) this.clock.add(ts.contextTime, ts.performanceTime);
			else this.clock.add(this.ctx.currentTime, performance.now());
		};
		sample();
		this.timer = setInterval(sample, 100);
	}

	private emit(c: MicChunk) {
		for (const l of this.listeners) l(c);
	}

	/** Receive raw chunks; returns an unsubscribe function. */
	onChunk(cb: (c: MicChunk) => void): () => void {
		this.listeners.add(cb);
		return () => this.listeners.delete(cb);
	}

	latency(): MicLatency {
		const outputMs = ((this.ctx as AudioContext & { outputLatency?: number }).outputLatency ?? 0) * 1000;
		const stats = (this.track as StatsTrack).stats;
		const fromStats = stats?.averageLatency ?? stats?.latency;
		if (typeof fromStats === 'number' && Number.isFinite(fromStats) && fromStats >= 0 && fromStats < 1000)
			return { inputMs: fromStats, outputMs, source: 'stats' };
		const base = this.ctx.baseLatency;
		if (typeof base === 'number' && Number.isFinite(base))
			return { inputMs: base * 1000, outputMs, source: 'base' };
		return { inputMs: 0, outputMs, source: 'none' };
	}

	/** Phone time (performance.now() ms) when the sound of context frame `frame` reached the mic. */
	frameToPerfMs(frame: number, lat: MicLatency = this.latency()): number {
		return this.clock.map(frame / this.ctx.sampleRate) - lat.outputMs - lat.inputMs;
	}

	close(): void {
		if (this.closed) return;
		this.closed = true;
		clearInterval(this.timer);
		this.listeners.clear();
		try {
			this.source.disconnect();
			this.node?.disconnect();
		} catch {
			/* already gone */
		}
		this.ctx.close().catch(() => {});
	}
}

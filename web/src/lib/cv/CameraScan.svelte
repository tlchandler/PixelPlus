<!--
	Camera scan for mapping (F6) and pixel counts (F7), WS4. Phone-first and dark-yard friendly:

	  camera off → framing (dim lights on, lock exposure, frame rate check) → scanning (progress,
	  live "what blinks" overlay, hold-still warning) → decoding (worker) → `ondone`.

	In demo mode (mock backend) there is no camera: a synthetic yard is rendered from the show's
	layout and "filmed" at 4× speed, so the whole flow can be tried (and tested) anywhere.
-->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import {
		Camera,
		Lock,
		LockOpen,
		Hand,
		TriangleAlert,
		Sparkles,
		Square,
		RotateCcw,
		Gauge
	} from '@lucide/svelte';
	import { app } from '$lib/stores/app.svelte';
	import { CameraCapture, describeMediaError, openMedia, stopStream } from '$lib/sensing/camera';
	import { keepAwake } from '$lib/sensing/device';
	import { MapRecorder, bitMsFor, decodeAsync, lockCamera, snapshot, type LockResult } from './capture';
	import { decode, type DecodeResult, type Recording } from './decode';
	import { mappingApi } from './api';
	import { demoScene, demoPhoto } from './demo';
	import { simulate } from './simulate';
	import type { MapStart } from './types';

	let {
		begin,
		ondone,
		oncancel,
		estimateMs,
		frameLights = true,
		startLabel = 'Start scan'
	}: {
		/** Start the pattern on the controllers for `bitMs` (returns the plan). */
		begin: (bitMs: number) => Promise<MapStart>;
		ondone: (r: {
			result: DecodeResult;
			start: MapStart;
			photo: Blob | null;
			photoUrl: string | null;
		}) => void;
		oncancel?: () => void;
		/** Estimated scan time for a bit length (ms), shown on the start button. */
		estimateMs?: (bitMs: number) => number;
		/** Light the display dim white while framing. */
		frameLights?: boolean;
		startLabel?: string;
	} = $props();

	type Stage = 'off' | 'starting' | 'framing' | 'scanning' | 'decoding' | 'error';
	let stage = $state<Stage>('off');
	let error = $state('');
	let video = $state<HTMLVideoElement>();
	let overlay = $state<HTMLCanvasElement>();
	let fps = $state(0);
	let lock = $state<LockResult | null>(null);
	let locking = $state(false);
	let progress = $state(0);
	let secondsLeft = $state(0);
	let shaking = $state(false);
	let busy = $state(false);

	let stream: MediaStream | null = null;
	let cam: CameraCapture | null = null;
	let recorder: MapRecorder | null = null;
	let release: (() => void) | null = null;
	let timer: ReturnType<typeof setInterval> | undefined;
	let fpsTimer: ReturnType<typeof setInterval> | undefined;
	let lightsOn = false;
	let aborted = false;
	let running: { start: MapStart; t0: number } | null = null;
	const demo = $derived(app.mock);

	const bitMs = $derived(bitMsFor(fps || 30));
	const estimate = $derived(estimateMs ? Math.ceil(estimateMs(bitMs) / 1000) : 0);

	async function openCamera() {
		error = '';
		stage = 'starting';
		try {
			if (!demo) {
				stream = await openMedia({ width: 1280, height: 720, frameRate: 30 });
				cam = new CameraCapture(stream, video);
				await cam.start();
				fpsTimer = setInterval(() => (fps = Math.round(cam?.fps ?? 0)), 500);
			} else fps = 30;
			release = await keepAwake().catch(() => null);
			if (frameLights) {
				await mappingApi.frame(true).catch(() => {});
				lightsOn = true;
			}
			stage = 'framing';
			if (!demo) {
				// Most phones settle exposure within a second; lock it for the user.
				setTimeout(() => {
					if (stage === 'framing' && !lock) void doLock();
				}, 1500);
			}
		} catch (e) {
			error = describeMediaError(e, 'camera').message;
			stage = 'error';
			cleanup();
		}
	}

	async function doLock() {
		if (!cam || locking) return;
		locking = true;
		lock = await lockCamera(cam).catch(() => ({ exposure: false, focus: false, whiteBalance: false }));
		locking = false;
	}

	function onMotion(e: DeviceMotionEvent) {
		const r = e.rotationRate;
		const deg = r ? Math.hypot(r.alpha ?? 0, r.beta ?? 0, r.gamma ?? 0) : 0;
		const now = deg > 12;
		shaking = now;
	}

	async function scan() {
		if (busy) return;
		busy = true;
		error = '';
		aborted = false;
		try {
			if (lightsOn) {
				await mappingApi.frame(false).catch(() => {});
				lightsOn = false;
			}
			// The photo for the review screen: the display lit dim white, as framed.
			const photo = !demo && video ? await snapshot(video).catch(() => null) : null;
			if (!demo && cam) {
				recorder = new MapRecorder(cam);
				recorder.start();
			}
			const t = performance.now();
			const start = await begin(bitMs);
			const rtt = performance.now() - t;
			// The pattern started while the request was answered.
			const startHint = performance.now() - rtt / 2;
			running = { start, t0: startHint };
			if (demo && app.show) {
				const scene = demoScene(app.show, start, DW, DH);
				demoRec = simulate(start.plan, scene, {
					width: DW,
					height: DH,
					fps: 24,
					noise: 2.5,
					patternStartMs: 1000,
					leadMs: 400
				});
			}
			stage = 'scanning';
			window.addEventListener('devicemotion', onMotion);
			const total = start.schedule.totalMs + 900;
			const speed = demo ? 4 : 1;
			timer = setInterval(() => {
				const el = (performance.now() - startHint) * speed;
				progress = Math.min(1, el / total);
				secondsLeft = Math.max(0, Math.ceil((total - el) / speed / 1000));
				drawOverlay(el);
				if (el >= total) void finish(photo, startHint);
			}, 200);
		} catch (e) {
			recorder?.stop();
			error = (e as Error).message;
			stage = 'error';
		} finally {
			busy = false;
		}
	}

	let demoRec: Recording | null = null;
	/** The demo camera films at the high-end phone size (480 wide). */
	const DW = 480,
		DH = 270;
	let demoUrl: string | null = null;

	function drawOverlay(el: number) {
		const c = overlay;
		if (!c) return;
		const ctx = c.getContext('2d');
		if (!ctx) return;
		if (demo) {
			// Play the synthetic recording as the "camera".
			const f = demoRec?.frames.find((x) => x.t >= 1000 + el) ?? demoRec?.frames.at(-1);
			if (!f) return;
			c.width = DW;
			c.height = DH;
			const img = ctx.createImageData(DW, DH);
			for (let i = 0; i < f.data.length; i++) {
				const v = f.data[i];
				img.data[i * 4] = v;
				img.data[i * 4 + 1] = v * 0.93;
				img.data[i * 4 + 2] = v * 0.8;
				img.data[i * 4 + 3] = 255;
			}
			ctx.putImageData(img, 0, 0);
			return;
		}
		if (!recorder) return;
		const s = recorder.swing();
		c.width = recorder.w;
		c.height = recorder.h;
		const img = ctx.createImageData(recorder.w, recorder.h);
		for (let i = 0; i < s.length; i++) {
			const v = s[i];
			if (v < 12) continue;
			const a = Math.min(255, v * 2);
			img.data[i * 4] = 255;
			img.data[i * 4 + 1] = 190;
			img.data[i * 4 + 2] = 60;
			img.data[i * 4 + 3] = a;
		}
		ctx.putImageData(img, 0, 0);
	}

	async function finish(photo: Blob | null, startHint: number) {
		if (stage !== 'scanning' || !running) return;
		clearInterval(timer);
		window.removeEventListener('devicemotion', onMotion);
		stage = 'decoding';
		const start = running.start;
		try {
			let result: DecodeResult;
			let photoUrl: string | null = null;
			if (demo) {
				const scene = demoScene(app.show!, start, DW, DH);
				const rec =
					demoRec ??
					simulate(start.plan, scene, {
						width: DW,
						height: DH,
						fps: 24,
						noise: 2.5,
						patternStartMs: 1000,
						leadMs: 400
					});
				// Decode on this thread in demo mode (the test browser has no worker bundling).
				result = await new Promise((r) => setTimeout(() => r(decode(rec, start.plan)), 30));
				demoUrl = await demoPhoto(scene, DW, DH);
				photoUrl = demoUrl;
			} else {
				const rec = recorder!.stop(startHint);
				recorder = null;
				result = await decodeAsync(rec, start.plan);
				if (photo) photoUrl = URL.createObjectURL(photo);
			}
			if (aborted) return;
			cleanup();
			stage = 'off';
			ondone({ result, start, photo, photoUrl });
		} catch (e) {
			error = (e as Error).message;
			stage = 'error';
		}
	}

	async function stopScan() {
		aborted = true;
		clearInterval(timer);
		window.removeEventListener('devicemotion', onMotion);
		recorder?.stop();
		recorder = null;
		if (running) await mappingApi.stop(running.start.runId).catch(() => {});
		running = null;
		stage = 'framing';
		progress = 0;
	}

	function cleanup() {
		clearInterval(timer);
		clearInterval(fpsTimer);
		window.removeEventListener('devicemotion', onMotion);
		if (lightsOn) {
			mappingApi.frame(false).catch(() => {});
			lightsOn = false;
		}
		cam?.stop();
		cam = null;
		stopStream(stream);
		stream = null;
		release?.();
		release = null;
		demoRec = null;
	}

	function cancel() {
		if (running && stage === 'scanning') mappingApi.stop(running.start.runId).catch(() => {});
		aborted = true;
		recorder?.stop();
		cleanup();
		stage = 'off';
		oncancel?.();
	}

	onDestroy(() => {
		if (stage === 'scanning' && running) mappingApi.stop(running.start.runId).catch(() => {});
		cleanup();
	});
</script>

<div class="scan" class:live={stage !== 'off' && stage !== 'error'}>
	<div class="viewport" class:demo>
		<video bind:this={video} playsinline muted class:hidden={demo || stage === 'off' || stage === 'error'}
		></video>
		<canvas bind:this={overlay} class="overlay" class:solo={demo} aria-hidden="true"></canvas>
		{#if stage === 'off' || stage === 'error'}
			<div class="placeholder">
				<span class="halo"><Camera size={28} /></span>
				{#if stage === 'error'}
					<p class="err" role="alert"><TriangleAlert size={16} /> {error}</p>
				{:else}
					<p class="muted">
						{demo
							? 'Demo: a simulated yard stands in for your camera.'
							: 'Use the back camera. Nothing is uploaded except the result and one still photo.'}
					</p>
				{/if}
			</div>
		{/if}
		{#if stage === 'framing'}
			<div class="frame-guide" aria-hidden="true"></div>
		{/if}
		{#if stage === 'scanning'}
			<div class="hud">
				<div class="bar"><span style:width="{progress * 100}%"></span></div>
				<div class="row between small">
					<span><Sparkles size={14} /> Reading the lights…</span>
					<span class="num">{secondsLeft} s</span>
				</div>
			</div>
			{#if shaking}
				<div class="shake" role="status"><Hand size={18} /> Phone moved — hold still</div>
			{/if}
		{/if}
		{#if stage === 'decoding'}
			<div class="placeholder dim">
				<span class="spinner" aria-hidden="true"></span>
				<p>Working out where every light is…</p>
			</div>
		{/if}
	</div>

	{#if stage === 'framing'}
		<ol class="tips small">
			<li>Lean the phone on something (a mailbox, a car roof) so the whole display fits in the picture.</li>
			<li>Keep it still for the whole scan — about {estimate || 30} seconds.</li>
		</ol>
		<div class="chips">
			{#if !demo}
				<button class="chip lockchip" class:on={lock?.exposure} onclick={doLock} disabled={locking}>
					{#if lock?.exposure}<Lock size={14} /> Exposure locked{:else}<LockOpen size={14} />
						{locking ? 'Locking…' : 'Lock exposure'}{/if}
				</button>
			{/if}
			<span class="chip" title="Camera frames per second"><Gauge size={14} /> {fps || '—'} fps</span>
			{#if fps && fps < 20}<span class="chip warn">Slow camera: using longer blinks</span>{/if}
			{#if lock && !lock.exposure && lock.note}<span class="chip faint" title={lock.note}
					>Auto exposure (still works)</span
				>{/if}
		</div>
	{/if}

	<div class="actions">
		{#if stage === 'off' || stage === 'error'}
			{#if oncancel}<button class="btn ghost lg" onclick={cancel}>Back</button>{/if}
			<button class="btn primary lg grow" onclick={openCamera}>
				{#if stage === 'error'}<RotateCcw size={18} /> Try again{:else}<Camera size={18} />
					{demo ? 'Start the demo camera' : 'Start camera'}{/if}
			</button>
		{:else if stage === 'starting'}
			<button class="btn primary lg grow" disabled>Starting the camera…</button>
		{:else if stage === 'framing'}
			<button class="btn ghost lg" onclick={cancel}>Cancel</button>
			<button class="btn primary lg grow" onclick={scan} disabled={busy}>
				<Sparkles size={18} />
				{startLabel}{estimate ? ` · about ${estimate} s` : ''}
			</button>
		{:else if stage === 'scanning'}
			<button class="btn lg grow" onclick={stopScan}><Square size={16} /> Stop</button>
		{/if}
	</div>
</div>

<style>
	.scan {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.viewport {
		position: relative;
		width: 100%;
		aspect-ratio: 16 / 9;
		border-radius: 16px;
		overflow: hidden;
		background: #030305;
		border: 1px solid var(--border-2);
	}
	.live .viewport {
		box-shadow: 0 0 0 1px rgba(245, 165, 36, 0.15);
	}
	video,
	.overlay {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.overlay {
		pointer-events: none;
		image-rendering: pixelated;
		mix-blend-mode: screen;
	}
	.overlay.solo {
		mix-blend-mode: normal;
	}
	.hidden {
		visibility: hidden;
	}
	.placeholder {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 10px;
		padding: 16px;
		text-align: center;
		color: #d8d8de;
	}
	.placeholder.dim {
		background: rgba(3, 3, 5, 0.72);
	}
	.placeholder .muted {
		color: #9a9aa6;
		max-width: 34ch;
	}
	.halo {
		width: 56px;
		height: 56px;
		border-radius: 18px;
		display: grid;
		place-items: center;
		background: rgba(245, 165, 36, 0.14);
		color: #f5a524;
	}
	.err {
		color: #ff8a8e;
		display: flex;
		gap: 6px;
		align-items: flex-start;
		max-width: 40ch;
	}
	.frame-guide {
		position: absolute;
		inset: 8%;
		border: 1.5px dashed rgba(255, 255, 255, 0.35);
		border-radius: 12px;
	}
	.hud {
		position: absolute;
		left: 10px;
		right: 10px;
		bottom: 10px;
		padding: 10px 12px;
		border-radius: 12px;
		background: rgba(5, 5, 8, 0.72);
		color: #ececef;
		display: flex;
		flex-direction: column;
		gap: 8px;
		backdrop-filter: blur(6px);
	}
	.hud span {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.bar {
		height: 6px;
		border-radius: 3px;
		background: rgba(255, 255, 255, 0.12);
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: #f5a524;
		transition: width 200ms linear;
	}
	.shake {
		position: absolute;
		top: 10px;
		left: 50%;
		transform: translateX(-50%);
		padding: 8px 14px;
		border-radius: 999px;
		background: #ff6b70;
		color: #1a0003;
		font-weight: 600;
		display: flex;
		gap: 6px;
		align-items: center;
		white-space: nowrap;
	}
	.spinner {
		width: 34px;
		height: 34px;
		border-radius: 50%;
		border: 3px solid rgba(255, 255, 255, 0.15);
		border-top-color: #f5a524;
		animation: spin 0.9s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.spinner {
			animation-duration: 3s;
		}
	}
	.tips {
		margin: 0;
		padding-left: 20px;
		color: var(--text-2);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.lockchip {
		cursor: pointer;
		min-height: 36px;
	}
	.lockchip.on {
		background: var(--green-soft);
		color: var(--green);
	}
	.chip.warn {
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.actions {
		display: flex;
		gap: 8px;
	}
	.actions .grow {
		flex: 1;
	}
</style>

<!--
	"Sync lights to sound" measured with the phone (F1, ARCHITECTURE §12.1). WS1.
	Steps: where are you → allow camera & mic → aim (live meters) → measuring → result (apply /
	undo / verify). Optional: fine-tune for this phone (clap test).
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import {
		AudioWaveform,
		Camera,
		Check,
		Hand,
		Radio,
		RotateCcw,
		SlidersHorizontal,
		TriangleAlert,
		Ear,
		Eye,
		Sparkles,
		X,
		Info,
		ShieldCheck
	} from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SecureGate from '$lib/components/ui/SecureGate.svelte';
	import ProgressRing from '$lib/components/calibrate/ProgressRing.svelte';
	import ClapTest from '$lib/components/calibrate/ClapTest.svelte';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { SensingSession, type LiveReadings } from '$lib/sensing/session';
	import { calibrationApi, type CalibrationApplied } from '$lib/sensing/api';
	import { computeOutcome, verified, VERIFY_LIMIT_MS, type MeasureOutcome } from '$lib/sensing/calibrate';
	import { MEASURE_EVENTS, measureSpanMs, type CalPlan } from '$lib/sensing/schedule';
	import { deviceModel, keepAwake, loadBias, type StoredBias } from '$lib/sensing/device';
	import { describeMediaError } from '$lib/sensing/camera';

	type Step = 'intro' | 'aim' | 'measuring' | 'result' | 'clap';

	let step = $state<Step>('intro');
	let radio = $state(true);
	let starting = $state(false);
	let error = $state<string | null>(null);
	let live = $state<LiveReadings | null>(null);
	let progress = $state(0);
	let secondsLeft = $state(0);
	let outcome = $state<MeasureOutcome | null>(null);
	let verifying = $state(false);
	let wasVerify = $state(false);
	let applying = $state(false);
	let applied = $state<CalibrationApplied | null>(null);
	let model = $state('This phone');
	let bias = $state<StoredBias | null>(null);
	let aimForce = $state(false);
	let video = $state<HTMLVideoElement>();

	let session = $state.raw<SensingSession | null>(null);
	let plan: CalPlan | null = null;
	let pos0: number | undefined;
	let patternOn = false;
	let measureStart = 0;
	let measureEnd = 0;
	let delayAtStart = $state(0);
	let tick: ReturnType<typeof setInterval> | undefined;
	let aimTimer: ReturnType<typeof setTimeout> | undefined;
	let release: () => void = () => {};

	const currentDelay = $derived(app.show?.settings.audio.outputDelayMs ?? 0);
	const lastCal = $derived(app.show?.settings.audio.lastCalibration);
	const follower = $derived(app.system?.role === 'follower');

	onMount(() => {
		void deviceModel().then((m) => {
			model = m;
			bias = loadBias(m);
		});
		const onHide = () => {
			if (patternOn) calibrationApi.stopBeacon();
		};
		window.addEventListener('pagehide', onHide);
		return () => window.removeEventListener('pagehide', onHide);
	});

	onDestroy(() => {
		clearInterval(tick);
		clearTimeout(aimTimer);
		release();
		if (patternOn) calibrationApi.stopBeacon();
		session?.close();
	});

	async function startPattern() {
		const s = await calibrationApi.start();
		plan = s.plan;
		pos0 = s.pos0Ms;
		patternOn = true;
		session?.setPlan(s.plan);
	}

	async function stopPattern() {
		if (!patternOn) return;
		patternOn = false;
		await calibrationApi.stop().catch(() => {});
	}

	async function begin() {
		error = null;
		starting = true;
		try {
			if (!session) {
				if (!video) throw new Error('The camera view is not ready.');
				session = await SensingSession.open(video);
			}
		} catch (e) {
			error = describeMediaError(e).message;
			session?.close();
			session = null;
			starting = false;
			return;
		}
		try {
			await startPattern();
			step = 'aim';
			aimForce = false;
			clearTimeout(aimTimer);
			aimTimer = setTimeout(() => (aimForce = true), 10_000);
			startTicking();
		} catch (e) {
			error = `Couldn't start the flashes and clicks: ${(e as Error).message}`;
		} finally {
			starting = false;
		}
	}

	function startTicking() {
		clearInterval(tick);
		tick = setInterval(() => {
			if (!session) return;
			live = session.live();
			if (step === 'measuring') {
				const now = performance.now();
				progress = (now - measureStart) / (measureEnd - measureStart);
				secondsLeft = Math.max(0, Math.ceil((measureEnd - now) / 1000));
				if (now >= measureEnd) void finish();
			}
		}, 250);
	}

	async function measure(verify = false) {
		if (!session) return begin();
		error = null;
		outcome = null;
		verifying = verify;
		starting = true;
		try {
			// A fresh run (new pattern from its start) for every measurement.
			await startPattern();
		} catch (e) {
			error = `Couldn't start the flashes and clicks: ${(e as Error).message}`;
			starting = false;
			return;
		}
		starting = false;
		delayAtStart = currentDelay;
		measureStart = performance.now();
		const p = plan!;
		const untilPos0 = pos0 !== undefined ? Math.max(0, pos0 - measureStart) : 800;
		const lateSound = radio ? 2300 : 900;
		measureEnd =
			measureStart +
			untilPos0 +
			measureSpanMs(p, MEASURE_EVENTS) +
			Math.max(0, delayAtStart) +
			lateSound +
			600;
		progress = 0;
		step = 'measuring';
		release = await keepAwake();
		startTicking();
	}

	async function finish() {
		if (!session || !plan || step !== 'measuring') return;
		const end = performance.now();
		release();
		release = () => {};
		const audioMs = session.clicksBetween(measureStart, end);
		const videoMs = session.flashesBetween(measureStart, end);
		outcome = computeOutcome({
			plan,
			audioMs,
			videoMs,
			windowStartMs: measureStart,
			windowEndMs: end,
			pos0Ms: pos0,
			currentDelayMs: delayAtStart,
			biasMs: bias?.biasMs,
			radio
		});
		wasVerify = verifying;
		verifying = false;
		step = 'result';
		void stopPattern();
	}

	function cancel() {
		release();
		release = () => {};
		void stopPattern();
		step = 'aim';
		void startPattern().catch(() => {});
	}

	async function apply() {
		if (!outcome?.ok) return;
		applying = true;
		try {
			const r = await calibrationApi.result({
				residualMs: outcome.residualMs,
				spreadMs: outcome.spreadMs,
				matches: outcome.matches,
				device: model,
				apply: true
			});
			applied = r;
			await app.reloadShow();
			toasts.success(`Lights now wait ${r.outputDelayMs} ms for the sound`, { label: 'Undo', run: undo });
		} catch (e) {
			toasts.error("Couldn't save the sound delay", (e as Error).message);
		} finally {
			applying = false;
		}
	}

	async function undo() {
		const a = applied;
		if (!a) return;
		try {
			await calibrationApi.undo(a.previousDelayMs, a.previousCalibration);
			applied = null;
			await app.reloadShow();
			toasts.info(`Back to ${a.previousDelayMs} ms`);
		} catch (e) {
			toasts.error("Couldn't undo", (e as Error).message);
		}
	}

	async function openClap() {
		await stopPattern();
		if (!session) {
			starting = true;
			try {
				if (!video) throw new Error('The camera view is not ready.');
				session = await SensingSession.open(video);
				startTicking();
			} catch (e) {
				error = describeMediaError(e).message;
				starting = false;
				return;
			}
			starting = false;
		}
		step = 'clap';
	}

	function clapDone(b: StoredBias | null) {
		bias = b;
		step = 'intro';
	}

	function signed(ms: number) {
		const r = Math.round(ms);
		return r === 0 ? '0 ms' : `${r > 0 ? '+' : '−'}${Math.abs(r)} ms`;
	}

	const badge = $derived(
		outcome?.confidence === 'excellent'
			? { cls: 'green', label: 'Excellent' }
			: outcome?.confidence === 'good'
				? { cls: 'blue', label: 'Good' }
				: { cls: 'accent', label: 'Measure again' }
	);
	const showVideo = $derived(step === 'aim' || step === 'measuring' || step === 'clap');
	const readyToMeasure = $derived(!!live?.seeingFlashes && !!live?.hearingClicks);
</script>

<svelte:head><title>Sync lights to sound · PixelPlus</title></svelte:head>

<div class="page cal">
	<PageHeader
		title="Sync lights to sound"
		subtitle="Measure the sound delay with your phone's camera and microphone."
	/>

	{#if follower}
		<div class="notice warn">
			<Info size={18} />
			<div>
				This controller follows your show leader. Open <strong>Sync lights to sound</strong> on the leader.
			</div>
		</div>
	{:else}
		<SecureGate need="camera and microphone" purpose="measure the sound delay">
			<section class="card stage">
				<!-- One camera view for every step, so the camera keeps running between measurements. -->
				<div class="viewfinder" class:hidden={!showVideo} class:small={step === 'measuring'}>
					<video bind:this={video} muted playsinline aria-label="Camera view"></video>
					{#if live && step === 'aim'}
						<div class="meters">
							<span class="meter" class:ok={live.seeingFlashes}>
								{#if live.seeingFlashes}<Check size={14} />{:else}<Eye size={14} />{/if}
								{live.seeingFlashes ? 'Seeing flashes' : 'Looking for flashes…'}
							</span>
							<span class="meter" class:ok={live.hearingClicks}>
								{#if live.hearingClicks}<Check size={14} />{:else}<Ear size={14} />{/if}
								{live.hearingClicks ? 'Hearing clicks' : 'Listening for clicks…'}
							</span>
						</div>
						<div class="bright" aria-hidden="true">
							<span style="width:{Math.min(100, Math.round(Math.sqrt(live.brightness) * 100))}%"></span>
						</div>
					{/if}
				</div>

				{#if step === 'intro'}
					<div class="body">
						<div class="hero"><AudioWaveform size={28} strokeWidth={1.6} /></div>
						<h2>Where are you?</h2>
						<p class="muted">
							Stand where your visitors listen, with the lights in view, and tune the radio or phone to your
							show's sound. The lights flash and click together for about 25 seconds while your phone watches
							and listens.
						</p>
						<label class="check">
							<input type="checkbox" bind:checked={radio} />
							<span>
								<span class="title"><Radio size={15} /> The sound comes from a radio / FM</span>
								<span class="faint small"
									>Or a streaming app or Bluetooth speaker: allows delays up to 2 seconds.</span
								>
							</span>
						</label>
						<div class="facts">
							<div class="fact">
								<span class="faint small">Sound delay now</span>
								<span class="num">{signed(currentDelay)}</span>
							</div>
							{#if lastCal}
								<div class="fact">
									<span class="faint small">Last measured</span>
									<span class="small"
										>{new Date(lastCal.measuredAt).toLocaleDateString()}{lastCal.device
											? ` · ${lastCal.device}`
											: ''}</span
									>
								</div>
							{/if}
						</div>
						<div class="notice info small">
							<ShieldCheck size={16} />
							<div>
								Your phone will ask to use the camera and microphone. Nothing is recorded or sent — only the
								result is saved.
							</div>
						</div>
						{#if error}<div class="notice danger small" role="alert">
								<TriangleAlert size={16} />
								<div>{error}</div>
							</div>{/if}
						{#if app.mock}
							<div class="notice warn small">
								<Info size={16} />
								<div>This is the demo: there are no real lights here to measure.</div>
							</div>
						{/if}
						<div class="actions">
							<button class="btn primary lg block" onclick={begin} disabled={starting}>
								<Camera size={18} />
								{starting ? 'Starting…' : 'Allow camera & microphone'}
							</button>
							<div class="row wrap secondary">
								<button class="btn ghost" onclick={openClap} disabled={starting}>
									<Hand size={16} />
									{bias ? `Fine-tuned for this phone (${signed(bias.biasMs)})` : 'Fine-tune for this phone'}
								</button>
								<a class="btn ghost" href="/settings#audio"><SlidersHorizontal size={16} /> Adjust by hand</a>
							</div>
						</div>
					</div>
				{:else if step === 'aim'}
					<div class="body">
						<h2>Aim at the lights</h2>
						<p class="muted">
							Point at as many lights as you can. Hold still, or lean the phone on something. The lights flash
							and click together.
						</p>
						{#if live && !live.seeingFlashes && aimForce}
							<p class="hint small">
								<Eye size={14} /> Can't see flashes yet — get more lights in view, or move closer.
							</p>
						{/if}
						{#if live && !live.hearingClicks && aimForce}
							<p class="hint small">
								<Ear size={14} /> Can't hear clicks yet — turn the radio or speaker up.
							</p>
						{/if}
						{#if error}<div class="notice danger small" role="alert">
								<TriangleAlert size={16} />
								<div>{error}</div>
							</div>{/if}
						<div class="actions">
							<button
								class="btn primary lg block"
								onclick={() => measure(false)}
								disabled={starting || (!readyToMeasure && !aimForce)}
							>
								<Sparkles size={18} />
								{readyToMeasure
									? 'Start measuring'
									: aimForce
										? 'Start anyway'
										: 'Waiting for flashes and clicks…'}
							</button>
							<button
								class="btn ghost"
								onclick={() => {
									void stopPattern();
									step = 'intro';
								}}>Back</button
							>
						</div>
					</div>
				{:else if step === 'measuring'}
					<div class="body measuring">
						<ProgressRing value={progress} label="Measuring">
							<div>
								<div class="big num">{secondsLeft}s</div>
								<div class="faint small">{verifying ? 'Checking' : 'Measuring'}</div>
							</div>
						</ProgressRing>
						<p class="muted center">Keep still — {MEASURE_EVENTS} flashes and clicks.</p>
						{#if live}
							<p class="faint small center">
								{live.flashesSeen} flashes and {live.clicksHeard} clicks in the last few seconds
							</p>
						{/if}
						<button class="btn ghost" onclick={cancel}><X size={16} /> Cancel</button>
					</div>
				{:else if step === 'result' && outcome}
					<div class="body result">
						{#if outcome.ok && wasVerify}
							<div class="hero" class:good={verified(outcome)}>
								{#if verified(outcome)}<Check size={28} />{:else}<TriangleAlert size={28} />{/if}
							</div>
							<h2>
								{verified(outcome)
									? 'Lights and sound match'
									: `Still ${Math.abs(Math.round(outcome.residualMs))} ms off`}
							</h2>
							<p class="muted">
								{#if verified(outcome)}
									The remaining difference is {Math.abs(Math.round(outcome.residualMs))} ms — well under what anyone
									can notice.
								{:else}
									More than the {VERIFY_LIMIT_MS} ms we aim for. Apply the correction, or measure again from a steadier
									spot.
								{/if}
							</p>
						{:else if outcome.ok}
							<span class="badge {badge.cls}">{badge.label}</span>
							<p class="lead">
								Sound reaches you <strong class="num">{Math.round(outcome.suggestedDelayMs)} ms</strong> after
								it leaves the controller <span class="faint">(±{outcome.uncertaintyMs} ms)</span>.
							</p>
							<p class="muted small">
								{#if applied}
									Lights are now delayed to match.
								{:else if Math.abs(outcome.residualMs) < 3}
									Your current setting is already right.
								{:else}
									That's {signed(outcome.residualMs)} from the current setting of {signed(delayAtStart)}.
								{/if}
								Sound travels about 3 ms per metre, so this is right for where you're standing.
							</p>
							{#if outcome.confidence === 'retry'}
								<div class="notice warn small">
									<Info size={16} />
									<div>
										The readings varied quite a bit. Measuring again from a steadier spot will give a better
										number.
									</div>
								</div>
							{/if}
						{:else}
							<div class="hero warn"><TriangleAlert size={28} /></div>
							<h2>Couldn't measure it this time</h2>
							<p class="muted">{outcome.hint}</p>
						{/if}
						{#if outcome.ok && outcome.clamped}
							<div class="notice warn small">
								<Info size={16} />
								<div>
									That's beyond what PixelPlus can delay (−0.5 to 2 s), so it will use the nearest limit.
								</div>
							</div>
						{/if}
						{#if outcome.limitedRange}
							<div class="notice info small">
								<Info size={16} />
								<div>
									Your controller uses the classic pattern, which can only tell delays apart within ±0.5 s.
									Update PixelPlus for the full range.
								</div>
							</div>
						{/if}

						<div class="actions">
							{#if outcome.ok && !applied && (!wasVerify || !verified(outcome))}
								<button class="btn primary lg block" onclick={apply} disabled={applying}>
									<Check size={18} />
									{applying ? 'Saving…' : 'Apply'}
								</button>
							{/if}
							{#if applied}
								<button class="btn primary lg block" onclick={() => measure(true)} disabled={starting}>
									<Sparkles size={18} /> Check it (optional)
								</button>
							{/if}
							<div class="row wrap secondary">
								<button
									class="btn"
									class:primary={!outcome.ok}
									onclick={() => {
										applied = null;
										void measure(false);
									}}
									disabled={starting}
								>
									<RotateCcw size={16} /> Measure again
								</button>
								{#if applied}
									<button class="btn ghost" onclick={undo}><RotateCcw size={16} /> Undo</button>
								{/if}
								<a class="btn ghost" href="/settings#audio"><SlidersHorizontal size={16} /> Adjust by hand</a>
							</div>
						</div>

						<details class="details">
							<summary class="faint small">Details</summary>
							<dl class="small">
								<dt>Clicks heard</dt>
								<dd>
									{outcome.audio.matched} of {outcome.audio.expected} ({outcome.audio.detections} sounds)
								</dd>
								<dt>Flashes seen</dt>
								<dd>{outcome.video.matched} of {outcome.video.expected}</dd>
								<dt>Spread</dt>
								<dd>{Number.isFinite(outcome.spreadMs) ? `${outcome.spreadMs.toFixed(1)} ms` : '—'}</dd>
								<dt>Phone</dt>
								<dd>{model}{bias ? `, fine-tuned (${signed(bias.biasMs)})` : ''}</dd>
								{#if live}
									<dt>Camera</dt>
									<dd>
										{Math.round(live.fps)} fps{live.captureTimes
											? ', sensor timestamps'
											: ''}{live.exposureLocked ? ', exposure fixed' : ''}
									</dd>
									<dt>Microphone latency</dt>
									<dd>
										{Math.round(live.micLatency.inputMs + live.micLatency.outputMs)} ms ({live.micLatency
											.source === 'stats'
											? 'reported'
											: 'estimated'})
									</dd>
								{/if}
							</dl>
						</details>
					</div>
				{:else if step === 'clap' && session}
					<ClapTest {session} {model} current={bias} ondone={clapDone} />
				{/if}
			</section>
		</SecureGate>
	{/if}
</div>

<style>
	.cal {
		max-width: 640px;
	}
	.stage {
		overflow: hidden;
	}
	.viewfinder {
		position: relative;
		background: var(--canvas-bg);
		aspect-ratio: 4 / 3;
		width: 100%;
		transition: max-height 300ms var(--ease);
		max-height: 60vh;
	}
	.viewfinder.small {
		max-height: 30vh;
	}
	.viewfinder.hidden {
		display: none;
	}
	video {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}
	.meters {
		position: absolute;
		left: 10px;
		right: 10px;
		bottom: 18px;
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
	}
	.meter {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 6px 10px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.62);
		color: #fff;
		font-size: 13px;
		font-weight: 550;
		backdrop-filter: blur(6px);
	}
	.meter.ok {
		background: rgba(18, 140, 90, 0.85);
	}
	.bright {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		height: 5px;
		background: rgba(0, 0, 0, 0.5);
	}
	.bright span {
		display: block;
		height: 100%;
		background: var(--accent);
		transition: width 240ms var(--ease);
	}
	.body {
		padding: var(--s-5);
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.measuring,
	.result {
		align-items: center;
		text-align: center;
	}
	.result .notice,
	.result .actions,
	.result .details {
		align-self: stretch;
		text-align: left;
	}
	.hero {
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.hero.good {
		background: var(--green-soft);
		color: var(--green);
	}
	h2 {
		font-size: 19px;
	}
	.lead {
		font-size: 18px;
		line-height: 1.45;
		max-width: 30em;
	}
	.lead strong {
		font-size: 30px;
		display: inline-block;
	}
	.check {
		display: flex;
		gap: 12px;
		align-items: flex-start;
		padding: 12px 14px;
		border: 1px solid var(--border-2);
		border-radius: var(--r-2);
		background: var(--surface-2);
		cursor: pointer;
		min-height: var(--touch);
	}
	.check input {
		width: 20px;
		height: 20px;
		margin-top: 1px;
		accent-color: var(--accent);
		flex: 0 0 auto;
	}
	.check > span {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.title {
		font-weight: 600;
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.facts {
		display: flex;
		gap: 24px;
		flex-wrap: wrap;
	}
	.fact {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.fact .num {
		font-size: 18px;
		font-weight: 650;
	}
	.actions {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 4px;
	}
	.secondary {
		gap: 8px;
		justify-content: center;
	}
	.hint {
		display: flex;
		gap: 6px;
		align-items: center;
		color: var(--accent-text);
		margin: 0;
	}
	.big {
		font-size: 28px;
		font-weight: 700;
	}
	.center {
		text-align: center;
	}
	.details {
		margin-top: 4px;
	}
	.details summary {
		cursor: pointer;
		min-height: var(--touch);
		display: flex;
		align-items: center;
	}
	dl {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 4px 16px;
		margin: 4px 0 0;
	}
	dt {
		color: var(--text-3);
	}
	dd {
		margin: 0;
	}
	@media (max-width: 760px) {
		.body {
			padding: var(--s-4);
		}
	}
</style>

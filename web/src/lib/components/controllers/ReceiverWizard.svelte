<!--
	Guided "Add receiver" (F9, WS4; ARCHITECTURE §12.8), phone-first:
	  controller → plug it in → find the jack (blink signals) → name → per port: which prop lit,
	  which end starts, colours right? → summary → create (auto snapshot, Undo).
	Signals are white or blue blinking 1–4 times, which read the same whatever the strip's colour
	order; the colour check then detects the order from two taps.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		Radio,
		Plug,
		Search,
		Check,
		ChevronLeft,
		CircleHelp,
		Lightbulb,
		ArrowRight,
		Palette,
		EyeOff,
		Undo2
	} from '@lucide/svelte';
	import type { Id, Node, ReceiverKind } from '$lib/api/types';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { app } from '$lib/stores/app.svelte';
	import { toasts } from '$lib/stores/toasts.svelte';
	import { api } from '$lib/api/client';
	import { BOARDS, RECEIVERS } from '$lib/util/boards';
	import { wizardApi, type PortPlan } from '$lib/cv/api';
	import type { IdentifyAnswer, JackSignal } from '$lib/cv/types';

	let { open = $bindable(false), nodeId: initialNode = null }: { open?: boolean; nodeId?: Id | null } =
		$props();

	type Step = 'node' | 'plug' | 'jack' | 'name' | 'port' | 'summary';
	let step = $state<Step>('node');
	let nodeId = $state<Id>('');
	let session = $state('');
	let ident = $state<IdentifyAnswer | null>(null);
	let probeIdx = $state(0);
	let jack = $state<number | null>(null);
	let busy = $state(false);
	let error = $state('');
	let nothingHelp = $state(false);

	let name = $state('');
	let location = $state('');
	let kind = $state<ReceiverKind>('diffrx');
	let fuse = $state<number | undefined>(6);

	type PortStage = 'which' | 'direction' | 'colour';
	let port = $state(1);
	let portStage = $state<PortStage>('which');
	let plans = $state<PortPlan[]>([]);
	let current = $state<PortPlan>({ port: 1, propIds: [], reverse: false });
	let several = $state(false);
	let filter = $state('');
	let redSeen = $state<string | null>(null);
	let colourNote = $state('');

	const show = $derived(app.show);
	const eligible = $derived(
		(show?.nodes ?? []).filter((n) => BOARDS[n.board]?.jacks > 0 || n.outputs.length >= 4)
	);
	const node = $derived<Node | undefined>(show?.nodes.find((n) => n.id === nodeId));
	const ports = $derived(RECEIVERS[kind]?.ports ?? 4);
	const usedNow = $derived(new Set(plans.flatMap((p) => p.propIds)));
	const candidates = $derived(
		(show?.props ?? [])
			.filter((p) => !usedNow.has(p.id) && !current.propIds.includes(p.id))
			.filter((p) => !filter || p.name.toLowerCase().includes(filter.toLowerCase()))
			.sort(
				(a, b) =>
					Number(a.segments.length > 0) - Number(b.segments.length > 0) || a.name.localeCompare(b.name)
			)
	);

	$effect(() => {
		// Only when the dialog opens (not on every show reload while it's open).
		if (open) untrack(begin);
	});

	function begin() {
		step = 'node';
		error = '';
		session = '';
		ident = null;
		jack = null;
		plans = [];
		port = 1;
		nodeId = initialNode ?? (eligible.length === 1 ? eligible[0].id : (eligible[0]?.id ?? ''));
		if (initialNode || eligible.length === 1) step = 'plug';
	}

	async function run<T>(f: () => Promise<T>): Promise<T | undefined> {
		busy = true;
		error = '';
		try {
			return await f();
		} catch (e) {
			error = (e as Error).message;
			return undefined;
		} finally {
			busy = false;
		}
	}

	async function identify() {
		const r = await run(() => wizardApi.identify(nodeId));
		if (!r) return;
		session = r.sessionId;
		ident = r;
		probeIdx = 0;
		step = 'jack';
	}

	async function pick(c: JackSignal) {
		const r = await run(() => wizardApi.pick(session, c.color, c.blinks));
		if (!r) return;
		if (r.done && r.jack) found(r.jack);
		else ident = { ...ident!, ...r };
	}

	async function sequential(yes: boolean) {
		if (!ident) return;
		const list = ident.candidates.map((c) => c.jack);
		if (yes) {
			const j = list[probeIdx];
			const r = await run(() => wizardApi.jack(session, j));
			if (r) found(j);
			return;
		}
		const next = probeIdx + 1;
		if (next >= list.length) {
			nothingHelp = true;
			return;
		}
		probeIdx = next;
		await run(() => wizardApi.probe(session, list[next]));
	}

	async function chooseJack(j: number) {
		const r = await run(() => wizardApi.jack(session, j));
		if (r) found(j);
	}

	function found(j: number) {
		jack = j;
		name = `Receiver J${j}`;
		step = 'name';
	}

	async function startPorts() {
		port = 1;
		plans = [];
		await lightPort(1);
	}

	async function lightPort(n: number) {
		port = n;
		portStage = 'which';
		current = { port: n, propIds: [], reverse: false };
		several = false;
		filter = '';
		redSeen = null;
		colourNote = '';
		step = 'port';
		await run(() => wizardApi.light(session, n, 'solid'));
	}

	async function chooseProp(id: Id) {
		if (several) {
			current.propIds = current.propIds.includes(id)
				? current.propIds.filter((x) => x !== id)
				: [...current.propIds, id];
			return;
		}
		current.propIds = [id];
		await toDirection();
	}

	async function toDirection() {
		portStage = 'direction';
		await run(() => wizardApi.light(session, port, 'chase'));
	}

	async function setDirection(fromController: boolean) {
		current.reverse = !fromController;
		portStage = 'colour';
		redSeen = null;
		await run(() => wizardApi.light(session, port, 'red'));
	}

	async function colour(seen: string) {
		if (!redSeen) {
			redSeen = seen;
			await run(() => wizardApi.light(session, port, 'green'));
			return;
		}
		if (seen === redSeen) {
			error = "Red and green can't look the same — let's try again.";
			redSeen = null;
			await run(() => wizardApi.light(session, port, 'red'));
			return;
		}
		const r = await run(() => wizardApi.colorOrder(session, port, redSeen!, seen));
		if (!r) return;
		if (r.changed) {
			current.colorOrder = r.colorOrder;
			colourNote = `Colour order set to ${r.colorOrder}.`;
		}
		await nextPort();
	}

	async function skipPort() {
		current.propIds = [];
		await nextPort();
	}

	async function nextPort() {
		if (current.propIds.length || current.colorOrder)
			plans = [...plans.filter((p) => p.port !== port), { ...current }];
		if (colourNote) toasts.info(colourNote);
		if (port < ports) await lightPort(port + 1);
		else {
			await run(() => wizardApi.light(session, port, 'off'));
			step = 'summary';
		}
	}

	async function finish() {
		const r = await run(() =>
			wizardApi.finish(session, {
				receiver: { name: name.trim(), kind, location: location.trim() || undefined, fuseAmps: fuse },
				ports: plans
			})
		);
		if (!r) return;
		session = '';
		await app.reloadShow();
		toasts.push({
			kind: 'success',
			message: `${name.trim()} added on J${jack}`,
			action: {
				label: 'Undo',
				run: async () => {
					await api.restoreSnapshot(r.snapshotId);
					await app.reloadShow();
				}
			}
		});
		open = false;
	}

	function close() {
		if (session) wizardApi.cancel(session).catch(() => {});
		session = '';
		open = false;
	}

	const propName = (id: Id) => show?.props.find((p) => p.id === id)?.name ?? id;
	const blinkStyle = (c: JackSignal) => `--c:${c.color === '#0000c0' ? '#3b6cff' : '#f2f2f2'}`;
	const colourLabel = (c: string) => (c === '#0000c0' ? 'Blue' : 'White');
	const title = $derived(
		step === 'port' ? `Port ${port} of ${ports}` : step === 'summary' ? 'Check and create' : 'Add a receiver'
	);
</script>

<Modal bind:open {title} subtitle={node ? node.name : undefined} size="md" onclose={close}>
	{#if step === 'node'}
		<div class="stack">
			<p class="muted">Which controller is the new receiver plugged into?</p>
			{#each eligible as n (n.id)}
				<label class="opt" class:on={nodeId === n.id}>
					<input type="radio" class="sr-only" bind:group={nodeId} value={n.id} />
					<Radio size={18} />
					<span class="grow"
						><strong>{n.name}</strong> <span class="faint small">{BOARDS[n.board]?.name}</span></span
					>
					{#if nodeId === n.id}<Check size={16} />{/if}
				</label>
			{:else}
				<p class="faint">No controller with receiver jacks yet.</p>
			{/each}
		</div>
	{:else if step === 'plug'}
		<div class="stack center">
			<span class="halo"><Plug size={26} /></span>
			<p>
				Plug the receiver's network cable into a <strong>free jack</strong> on {node?.name}, and power the
				receiver.
			</p>
			<p class="faint small">
				Next, port 1 of every free jack blinks its own signal. You tell PixelPlus which one you see.
			</p>
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
		</div>
	{:else if step === 'jack' && ident}
		<div class="stack">
			{#if ident.method === 'identify'}
				<p>Look at the lights on the new receiver's <strong>port 1</strong>. Which signal do you see?</p>
				<div class="signals">
					{#each [...new Map(ident.candidates.map( (c) => [`${c.color}${c.blinks}`, c] )).values()] as c (c.color + c.blinks)}
						<button
							class="signal"
							disabled={busy}
							onclick={() => pick(c)}
							aria-label="{colourLabel(c.color)}, {c.blinks} blink{c.blinks > 1 ? 's' : ''}"
						>
							<span class="bulb b{c.blinks}" style={blinkStyle(c)}></span>
							<span class="small">{colourLabel(c.color)} ×{c.blinks}</span>
						</button>
					{/each}
				</div>
				{#if ident.round === 'first'}<p class="faint small">
						Many free jacks: one more quick round after this.
					</p>{/if}
			{:else}
				<p>Is port 1 of the new receiver lit now?</p>
				<p class="faint small">
					Trying jack J{ident.candidates[probeIdx]?.jack} ({probeIdx + 1} of {ident.candidates.length}).
				</p>
			{/if}
			<button class="btn ghost sm help" onclick={() => (nothingHelp = !nothingHelp)}
				><EyeOff size={14} /> Nothing lights up</button
			>
			{#if nothingHelp}
				<div class="notice info small">
					<CircleHelp size={16} />
					<div>
						Check the receiver has power (its LED is on), the cable is fully clicked in at both ends, and that
						port 1 has a string attached. Or pick the jack yourself:
						<div class="jacks">
							{#each ident.candidates as c (c.jack)}<button class="btn sm" onclick={() => chooseJack(c.jack)}
									>J{c.jack}</button
								>{/each}
						</div>
					</div>
				</div>
			{/if}
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
		</div>
	{:else if step === 'name'}
		<form class="stack" id="rxname" onsubmit={(e) => (e.preventDefault(), startPorts())}>
			<div class="notice ok small"><Check size={16} /> Found it: jack J{jack}.</div>
			<label class="field"
				><span class="eyebrow">Name</span><input
					class="input"
					bind:value={name}
					required
					maxlength="60"
				/></label
			>
			<label class="field"
				><span class="eyebrow">Where is it? (optional)</span><input
					class="input"
					bind:value={location}
					placeholder="Front porch"
				/></label
			>
			<div class="grid-2">
				<label class="field">
					<span class="eyebrow">Type</span>
					<select class="select" bind:value={kind} onchange={() => (fuse = RECEIVERS[kind]?.fuse)}>
						{#each Object.entries(RECEIVERS) as [k, r] (k)}<option value={k}>{r.short}</option>{/each}
					</select>
				</label>
				<label class="field"
					><span class="eyebrow">Port fuses (A)</span><input
						class="input"
						type="number"
						min="1"
						max="40"
						step="0.5"
						bind:value={fuse}
					/></label
				>
			</div>
		</form>
	{:else if step === 'port'}
		<div class="stack">
			{#if portStage === 'which'}
				<p><Lightbulb size={16} /> Port {port} is lit white. <strong>Which prop lit up?</strong></p>
				<input class="input" placeholder="Find a prop" bind:value={filter} aria-label="Find a prop" />
				<div class="props">
					{#each current.propIds as id (id)}
						<button class="opt on" onclick={() => chooseProp(id)}><Check size={16} /> {propName(id)}</button>
					{/each}
					{#each candidates.slice(0, 40) as p (p.id)}
						<button class="opt" onclick={() => chooseProp(p.id)}>
							<span class="grow">{p.name}</span>
							<span class="faint small">{p.pixelCount} px{p.segments.length ? ' · wired elsewhere' : ''}</span
							>
						</button>
					{/each}
				</div>
				<label class="row small"
					><input type="checkbox" bind:checked={several} /> Several props are chained on this port (pick them in
					order)</label
				>
			{:else if portStage === 'direction'}
				<p>
					A light now runs along {current.propIds.map(propName).join(' → ')}.
					<strong>Where does it start?</strong>
				</p>
				<div class="big2">
					<button class="btn lg" onclick={() => setDirection(true)}>At the cable end</button>
					<button class="btn lg" onclick={() => setDirection(false)}>At the far end</button>
				</div>
			{:else}
				<p>
					<Palette size={16} /> The prop should now be <strong>{redSeen ? 'green' : 'red'}</strong>. What
					colour do you see?
				</p>
				<div class="swatches">
					{#each ['red', 'green', 'blue'] as c (c)}
						<button class="sw {c}" onclick={() => colour(c)} aria-label={c}><span></span>{c}</button>
					{/each}
				</div>
			{/if}
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
		</div>
	{:else if step === 'summary'}
		<div class="stack">
			<div class="sum">
				<div>
					<span class="faint small">Receiver</span><strong>{name}</strong>
					<span class="faint small">on {node?.name} J{jack}{location ? ` · ${location}` : ''}</span>
				</div>
				{#each Array(ports) as _, i (i)}
					{@const p = plans.find((x) => x.port === i + 1)}
					<div>
						<span class="faint small">Port {i + 1}</span>
						{#if p?.propIds.length}<strong>{p.propIds.map(propName).join(' → ')}</strong>
							<span class="faint small"
								>{p.reverse ? 'reversed' : ''}{p.colorOrder ? ` · ${p.colorOrder}` : ''}</span
							>
						{:else}<span class="faint">Nothing</span>{/if}
					</div>
				{/each}
			</div>
			<p class="faint small">A snapshot is taken first; Undo puts everything back.</p>
			{#if error}<p class="err small" role="alert">{error}</p>{/if}
		</div>
	{/if}

	{#snippet footer()}
		{#if step === 'node'}
			<button class="btn ghost" onclick={close}>Cancel</button>
			<button class="btn primary" disabled={!nodeId} onclick={() => (step = 'plug')}
				>Next <ArrowRight size={16} /></button
			>
		{:else if step === 'plug'}
			<button class="btn ghost" onclick={close}>Cancel</button>
			<button class="btn primary" disabled={busy} onclick={identify}
				><Search size={16} /> {busy ? 'Lighting…' : "It's plugged in"}</button
			>
		{:else if step === 'jack' && ident?.method === 'sequential'}
			<button class="btn lg" disabled={busy} onclick={() => sequential(false)}>No, try the next</button>
			<button class="btn primary lg" disabled={busy} onclick={() => sequential(true)}>Yes, it's lit</button>
		{:else if step === 'jack'}
			<button class="btn ghost" onclick={close}>Cancel</button>
		{:else if step === 'name'}
			<button class="btn ghost" onclick={() => (step = 'jack')}><ChevronLeft size={16} /> Back</button>
			<button class="btn primary" type="submit" form="rxname" disabled={!name.trim() || busy}
				>Next: its ports</button
			>
		{:else if step === 'port'}
			{#if portStage === 'which'}
				<button class="btn ghost" onclick={skipPort} disabled={busy}>Nothing lit</button>
				{#if several}<button
						class="btn primary"
						disabled={!current.propIds.length || busy}
						onclick={toDirection}>Next</button
					>{/if}
			{:else}
				<button class="btn ghost" onclick={() => lightPort(port)}
					><Undo2 size={16} /> Start this port again</button
				>
				{#if portStage === 'colour'}<button class="btn" onclick={nextPort}>Skip the colour check</button>{/if}
			{/if}
		{:else if step === 'summary'}
			<button class="btn ghost" onclick={() => lightPort(1)}><ChevronLeft size={16} /> Redo ports</button>
			<button class="btn primary" disabled={busy} onclick={finish}><Check size={16} /> Create receiver</button
			>
		{/if}
	{/snippet}
</Modal>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.center {
		align-items: center;
		text-align: center;
	}
	.halo {
		width: 60px;
		height: 60px;
		border-radius: 18px;
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-text);
	}
	.opt {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 12px;
		min-height: 48px;
		border-radius: 12px;
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		color: var(--text);
		text-align: left;
		cursor: pointer;
	}
	.opt.on {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}
	.opt .grow {
		flex: 1;
	}
	.props {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-height: 42vh;
		overflow: auto;
	}
	.signals {
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		gap: 8px;
	}
	.signal {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding: 14px 6px;
		border-radius: 14px;
		border: 1px solid var(--border-2);
		background: #07070a;
		color: #d8d8de;
		cursor: pointer;
		min-height: 88px;
	}
	.bulb {
		width: 22px;
		height: 22px;
		border-radius: 50%;
		background: var(--c);
		box-shadow: 0 0 12px var(--c);
		/* Mirrors mapcode::identify_on: n × (350 ms on + 350 ms off), then a 1.4 s pause. */
	}
	.bulb.b1 {
		animation: b1 2.1s steps(1) infinite;
	}
	.bulb.b2 {
		animation: b2 2.8s steps(1) infinite;
	}
	.bulb.b3 {
		animation: b3 3.5s steps(1) infinite;
	}
	.bulb.b4 {
		animation: b4 4.2s steps(1) infinite;
	}
	@keyframes b1 {
		0% {
			opacity: 1;
		}
		16.7% {
			opacity: 0.08;
		}
	}
	@keyframes b2 {
		0%,
		25% {
			opacity: 1;
		}
		12.5%,
		37.5% {
			opacity: 0.08;
		}
	}
	@keyframes b3 {
		0%,
		20%,
		40% {
			opacity: 1;
		}
		10%,
		30%,
		50% {
			opacity: 0.08;
		}
	}
	@keyframes b4 {
		0%,
		16.7%,
		33.3%,
		50% {
			opacity: 1;
		}
		8.3%,
		25%,
		41.7%,
		58.3% {
			opacity: 0.08;
		}
	}
	.help {
		align-self: flex-start;
	}
	.jacks {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 8px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.big2 {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 8px;
	}
	.swatches {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 8px;
	}
	.sw {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		padding: 12px;
		min-height: 80px;
		border-radius: 14px;
		border: 1px solid var(--border-2);
		background: var(--surface-2);
		color: var(--text);
		text-transform: capitalize;
		cursor: pointer;
	}
	.sw span {
		width: 26px;
		height: 26px;
		border-radius: 50%;
	}
	.sw.red span {
		background: #ff3030;
	}
	.sw.green span {
		background: #20d060;
	}
	.sw.blue span {
		background: #3060ff;
	}
	.sum {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.sum > div {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 10px;
		align-items: baseline;
		padding: 10px 12px;
		border-radius: 12px;
		background: var(--surface-2);
	}
	.sum .faint:first-child {
		width: 64px;
	}
	.err {
		color: var(--red);
	}
	@media (max-width: 640px) {
		.signals {
			grid-template-columns: repeat(2, 1fr);
		}
	}
</style>

<!--
	Settings → Sensors (F20, ARCHITECTURE §12.16). WS6.
	ESP32 sensor nodes in the yard: add the ones announcing themselves, name their inputs
	(motion, button, beam, contact, current), see them live, identify / remove. What a
	sensor does is set up as a trigger (Settings → Triggers).
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { ArrowLeft, Radar, Plus, Lightbulb, Trash2, Zap, Info, Save, RadioTower } from '@lucide/svelte';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import SignalBars from '$lib/components/ui/SignalBars.svelte';
	import type { DiscoveredSensorNode, SensorInput, SensorInputKind, SensorNode } from '$lib/api/types';
	import { app } from '$lib/stores/app.svelte';
	import { confirm, toasts } from '$lib/stores/toasts.svelte';
	import { fmtRelative } from '$lib/util/format';
	import { sensorsApi, type SensorLive } from '$lib/insight/api';

	const KINDS: { value: SensorInputKind; label: string }[] = [
		{ value: 'motion', label: 'Motion (PIR)' },
		{ value: 'button', label: 'Button' },
		{ value: 'beam', label: 'Light beam' },
		{ value: 'contact', label: 'Door / contact' },
		{ value: 'current', label: 'Current (INA219/226)' }
	];

	let discovered = $state<DiscoveredSensorNode[]>([]);
	let live = $state<Record<string, SensorLive>>({});
	let drafts = $state<Record<string, SensorNode>>({});
	let dirty = $state<Record<string, boolean>>({});
	let busy = $state('');
	let timer: ReturnType<typeof setInterval> | undefined;
	let unsub: (() => void) | undefined;

	const nodes = $derived(app.show?.sensorNodes ?? []);
	const triggersFor = (id: string) =>
		(app.show?.settings.triggers ?? []).filter((t) => t.kind === 'sensor' && t.sensor?.sensorNodeId === id);

	$effect(() => {
		// Keep drafts in step with the show unless being edited.
		for (const n of nodes) if (!dirty[n.id]) drafts[n.id] = structuredClone($state.snapshot(n)) as SensorNode;
	});

	async function poll() {
		try {
			[discovered, live] = await Promise.all([sensorsApi.discovered(), sensorsApi.live()]);
		} catch {
			/* offline: keep the last values */
		}
	}
	onMount(() => {
		void poll();
		timer = setInterval(poll, 4000);
		unsub = app.onMessage('sensorInput', (e) => {
			const l = live[e.sensorNodeId];
			if (l) l.inputs = { ...l.inputs, [e.input]: e.state };
		});
	});
	onDestroy(() => {
		clearInterval(timer);
		unsub?.();
	});

	async function adopt(d: DiscoveredSensorNode) {
		busy = d.id;
		try {
			const n = await sensorsApi.adopt(d.id);
			toasts.success(`Added ${n.name}`);
			await app.reloadShow();
			await poll();
		} catch (e) {
			toasts.error(`Couldn't add ${d.name}`, (e as Error).message);
		} finally {
			busy = '';
		}
	}

	async function save(id: string) {
		const d = drafts[id];
		busy = id;
		try {
			await sensorsApi.update(id, {
				name: d.name,
				location: d.location ?? null,
				inputs: d.inputs
			} as Partial<SensorNode>);
			dirty[id] = false;
			await app.reloadShow();
			toasts.success('Saved — the sensor picks it up within seconds');
		} catch (e) {
			toasts.error("Couldn't save", (e as Error).message);
		} finally {
			busy = '';
		}
	}

	async function release(n: SensorNode) {
		const uses = triggersFor(n.id).length;
		const ok = await confirm({
			title: `Remove ${n.name}?`,
			message: `The sensor forgets this show and can be added again or to another show.${uses ? ` ${uses} trigger${uses === 1 ? '' : 's'} using it will stop working.` : ''}`,
			confirmLabel: 'Remove',
			danger: true
		});
		if (!ok) return;
		busy = n.id;
		try {
			const r = await sensorsApi.release(n.id);
			if (r.message) toasts.warn(r.message);
			else toasts.success(`Removed ${n.name}`);
			dirty[n.id] = false;
			await app.reloadShow();
		} catch (e) {
			toasts.error("Couldn't remove it", (e as Error).message);
		} finally {
			busy = '';
		}
	}

	async function identify(n: SensorNode) {
		try {
			await sensorsApi.identify(n.id);
			toasts.info(`${n.name} is blinking its light`);
		} catch (e) {
			toasts.error("Couldn't reach it", (e as Error).message);
		}
	}

	function edit(id: string) {
		dirty[id] = true;
	}
	function addInput(id: string) {
		const d = drafts[id];
		let k = 1;
		while (d.inputs.some((i) => i.id === `in${k}`)) k++;
		d.inputs.push({
			id: `in${k}`,
			name: `Input ${k}`,
			pin: 4,
			kind: 'button',
			activeLow: true,
			debounceMs: 30,
			holdMs: 0
		});
		edit(id);
	}
	function setKind(id: string, inp: SensorInput, kind: SensorInputKind) {
		inp.kind = kind;
		if (kind === 'current') {
			inp.pin = 64;
			inp.shuntMilliohms ??= 100;
		} else delete inp.shuntMilliohms;
		if (kind === 'button' || kind === 'contact') inp.activeLow = true;
		edit(id);
	}
</script>

<svelte:head><title>Sensors · Settings · PixelPlus</title></svelte:head>

<div class="page sensors">
	<a class="btn ghost sm back" href="/settings"><ArrowLeft size={16} /> Settings</a>
	<PageHeader
		title="Sensors"
		subtitle="Motion sensors, buttons and light beams in the yard (ESP32 sensor nodes) — they can set off surprises."
	>
		{#snippet actions()}
			<a class="btn" href="/settings/triggers"><Zap size={16} /> Triggers</a>
		{/snippet}
	</PageHeader>

	{#if discovered.length}
		<section class="card found">
			<div class="card-head">
				<RadioTower size={18} />
				<h2 class="grow">New sensors found</h2>
			</div>
			<div class="list">
				{#each discovered as d (d.id)}
					<div class="list-row">
						<div class="grow" style="min-width:0">
							<div class="small">{d.name}</div>
							<div class="faint tiny">
								{d.hw} · v{d.ver} · {d.ip}{d.inputs.length ? ` · ${d.inputs.join(', ')}` : ''}
								{#if d.adoptedBy}· belongs to another show{/if}
							</div>
						</div>
						<button class="btn sm primary" onclick={() => adopt(d)} disabled={busy === d.id}
							><Plus size={14} /> Add</button
						>
					</div>
				{/each}
			</div>
		</section>
	{/if}

	{#if !nodes.length}
		<section class="card">
			<EmptyState
				icon={Radar}
				title="No sensors yet"
				message="Flash an ESP32 with the PixelPlus sensor firmware, join it to your Wi-Fi from its setup hotspot (PixelPlus-Sensor-XXXX), and it shows up here within a few seconds."
			/>
		</section>
	{/if}

	{#each nodes as n (n.id)}
		{@const d = drafts[n.id]}
		{@const l = live[n.id]}
		{#if d}
			<section class="card node">
				<div class="card-head">
					<span class="dot {l?.online ? 'green' : ''}" aria-hidden="true"></span>
					<input
						class="title-input grow"
						bind:value={d.name}
						oninput={() => edit(n.id)}
						aria-label="Sensor name"
						maxlength="40"
					/>
					{#if l?.online}<SignalBars dbm={l.rssi} />{/if}
					<span class="faint tiny"
						>{l?.online ? 'online' : l?.lastSeen ? `seen ${fmtRelative(l.lastSeen)}` : 'offline'}</span
					>
				</div>
				<div class="card-body col">
					<div class="row wrap meta faint small">
						<input
							class="input sm"
							style="max-width:260px"
							placeholder="Where is it? (Driveway)"
							value={d.location ?? ''}
							oninput={(e) => {
								d.location = e.currentTarget.value;
								edit(n.id);
							}}
							aria-label="Location"
						/>
						<span>{n.hw}{l?.ver ? ` · v${l.ver}` : ''}{l?.ip ? ` · ${l.ip}` : ''}</span>
						{#if l && l.rejected > 0}<span
								class="badge red"
								title="Datagrams with a wrong signature or replayed">{l.rejected} rejected</span
							>{/if}
					</div>
					<div class="table-wrap">
						<table class="table inputs">
							<thead>
								<tr
									><th>Now</th><th>Name</th><th>Kind</th><th>Pin</th><th>Active low</th><th>Debounce</th><th
										>Hold</th
									><th></th></tr
								>
							</thead>
							<tbody>
								{#each d.inputs as inp, i (i)}
									{@const cur = inp.kind === 'current'}
									<tr>
										<td>
											{#if cur}
												<span class="num small"
													>{l?.amps[inp.id] != null ? `${l.amps[inp.id].toFixed(2)} A` : '—'}</span
												>
											{:else}
												<span
													class="state"
													class:on={l?.inputs[inp.id] === 1}
													title={l?.inputs[inp.id] === 1 ? 'Active' : 'Idle'}
													>{l?.inputs[inp.id] === 1 ? 'Active' : 'Idle'}</span
												>
											{/if}
										</td>
										<td>
											<input
												class="input sm"
												bind:value={inp.name}
												oninput={() => edit(n.id)}
												aria-label="Input name"
											/>
											<div class="faint tiny mono">{inp.id}</div>
										</td>
										<td>
											<select
												class="select sm"
												value={inp.kind}
												onchange={(e) => setKind(n.id, inp, e.currentTarget.value as SensorInputKind)}
												aria-label="Kind"
											>
												{#each KINDS as k (k.value)}<option value={k.value}>{k.label}</option>{/each}
											</select>
										</td>
										<td>
											<input
												class="input sm num"
												style="width:72px"
												type="number"
												min={cur ? 64 : 0}
												max={cur ? 79 : 48}
												bind:value={inp.pin}
												oninput={() => edit(n.id)}
												aria-label={cur ? 'I²C address' : 'GPIO'}
												title={cur ? 'I²C address (64 = 0x40)' : 'GPIO number'}
											/>
											{#if cur}
												<div class="row tiny faint shunt">
													<input
														class="input sm num"
														style="width:72px"
														type="number"
														min="0.05"
														step="0.05"
														bind:value={inp.shuntMilliohms}
														oninput={() => edit(n.id)}
														aria-label="Shunt milliohms"
													/> mΩ
												</div>
											{/if}
										</td>
										<td>
											{#if !cur}<input
													type="checkbox"
													bind:checked={inp.activeLow}
													onchange={() => edit(n.id)}
													aria-label="Active low"
												/>{/if}
										</td>
										<td>
											{#if !cur}<input
													class="input sm num"
													style="width:70px"
													type="number"
													min="0"
													max="10000"
													bind:value={inp.debounceMs}
													oninput={() => edit(n.id)}
													aria-label="Debounce ms"
												/> <span class="faint tiny">ms</span>{/if}
										</td>
										<td>
											{#if !cur}<input
													class="input sm num"
													style="width:80px"
													type="number"
													min="0"
													max="600000"
													step="500"
													bind:value={inp.holdMs}
													oninput={() => edit(n.id)}
													aria-label="Hold ms"
												/> <span class="faint tiny">ms</span>{/if}
										</td>
										<td>
											<button
												class="btn sm ghost icon"
												aria-label="Remove input"
												onclick={() => {
													d.inputs.splice(i, 1);
													edit(n.id);
												}}><Trash2 size={14} /></button
											>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
					<div class="row wrap acts">
						<button class="btn sm ghost" onclick={() => addInput(n.id)} disabled={d.inputs.length >= 8}
							><Plus size={14} /> Input</button
						>
						<button class="btn sm ghost" onclick={() => identify(n)} disabled={!l?.online}
							><Lightbulb size={14} /> Identify</button
						>
						<a class="btn sm ghost" href="/settings/triggers"
							><Zap size={14} />
							{triggersFor(n.id).length
								? `${triggersFor(n.id).length} trigger${triggersFor(n.id).length === 1 ? '' : 's'}`
								: 'Add a trigger'}</a
						>
						<span class="grow"></span>
						<button class="btn sm ghost danger" onclick={() => release(n)} disabled={busy === n.id}
							><Trash2 size={14} /> Remove</button
						>
						<button class="btn sm primary" onclick={() => save(n.id)} disabled={!dirty[n.id] || busy === n.id}
							><Save size={14} /> Save</button
						>
					</div>
				</div>
			</section>
		{/if}
	{/each}

	<div class="notice info">
		<Info size={18} />
		<div class="small">
			Hold <i>debounce</i> short (30 ms) for buttons; for PIR motion sensors use a <i>hold</i> of a few seconds
			so one visitor makes one trigger. A current input measures a receiver's pixel supply (I²C address and shunt
			resistance from your INA module). Sensor nodes talk to the show leader on UDP port 32422, signed with their
			own key.
		</div>
	</div>
</div>

<style>
	.sensors {
		max-width: 1000px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.back {
		align-self: flex-start;
		margin-bottom: -8px;
	}
	.found {
		border-color: var(--accent-line);
	}
	.dot.green {
		background: var(--green);
	}
	.meta {
		gap: 12px;
		align-items: center;
	}
	.table-wrap {
		overflow-x: auto;
	}
	.inputs td {
		vertical-align: top;
	}
	.state {
		display: inline-block;
		min-width: 52px;
		padding: 2px 8px;
		border-radius: 999px;
		font-size: 11.5px;
		text-align: center;
		background: var(--surface-3);
		color: var(--text-2);
	}
	.state.on {
		background: var(--accent-soft);
		color: var(--accent-text);
		font-weight: 600;
	}
	.shunt {
		margin-top: 4px;
		gap: 4px;
		align-items: center;
	}
	.acts {
		gap: 6px;
		align-items: center;
	}
</style>

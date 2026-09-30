<!--
	Trigger list editor (WS6, F20): what starts a trigger (a button on the controller, a
	web link, or an ESP32 sensor input), what it does (play, a look, stop, or a *surprise*
	layered over the running song on chosen props), and its limits (cooldown, per-hour cap,
	when, time window). Edits `triggers` in place (the owner should also watch it deeply:
	the time-window pickers edit through bindings) and calls `onchange` after edits.
-->
<script lang="ts">
	import { Plus, Trash2, Play, ChevronDown, Sparkles, Zap, Radar, Link2, CircuitBoard } from '@lucide/svelte';
	import type { Show, Target, TimeSpec, Trigger, TriggerAction, TriggerWhen } from '$lib/api/types';
	import { newId } from '$lib/util/id';
	import { toasts } from '$lib/stores/toasts.svelte';
	import TimeSpecPicker from '$lib/components/schedule/TimeSpecPicker.svelte';
	import { sensorsApi } from '$lib/insight/api';
	import { isEnabled } from '$lib/features';

	let {
		triggers = $bindable(),
		show,
		onchange
	}: { triggers: Trigger[]; show: Show; onchange: () => void } = $props();

	let open = $state<Record<string, boolean>>({});
	// Settings → Features: sensor inputs and surprises are offered only while they're on
	// (an existing trigger keeps showing what it's set to).
	const sensorsOn = $derived(isEnabled('sensors'));
	const surprisesOn = $derived(isEnabled('surprises'));

	const WHEN: { value: TriggerWhen; label: string }[] = [
		{ value: 'always', label: 'Any time' },
		{ value: 'showOnly', label: 'Only during the show' },
		{ value: 'idleOnly', label: 'Only while the idle look runs' },
		{ value: 'offOnly', label: 'Only when the show is off' }
	];
	const sensorInputs = $derived(
		(show.sensorNodes ?? []).flatMap((n) =>
			n.inputs.filter((i) => i.kind !== 'current').map((i) => ({ node: n, input: i, key: `${n.id}/${i.id}` }))
		)
	);

	function add(kind: Trigger['kind']) {
		const t: Trigger = {
			id: newId(),
			name: kind === 'sensor' ? 'Sensor surprise' : 'New trigger',
			kind,
			action:
				kind === 'sensor'
					? {
							type: 'surprise',
							source: 'effect',
							ref: show.effects[0]?.id,
							target: { all: true, propIds: [], groupIds: [] },
							durationMs: 5000
						}
					: { type: 'playPlaylist', ref: show.playlists[0]?.id }
		};
		if (kind === 'gpio') t.gpio = 17;
		if (kind === 'sensor') {
			const s = sensorInputs[0];
			if (s) t.sensor = { sensorNodeId: s.node.id, input: s.input.id };
			t.cooldownS = 60;
		}
		triggers.push(t);
		open[t.id] = true;
		onchange();
	}

	function remove(t: Trigger) {
		const i = triggers.findIndex((x) => x.id === t.id);
		if (i >= 0) triggers.splice(i, 1);
		onchange();
	}

	function setType(a: TriggerAction, type: TriggerAction['type']) {
		a.type = type;
		if (type === 'surprise') {
			a.source ??= 'effect';
			a.target ??= { all: true, propIds: [], groupIds: [] };
			a.durationMs ??= 5000;
			a.ref = (a.source === 'sequence' ? show.sequences : show.effects)[0]?.id;
		} else {
			delete a.target;
			delete a.durationMs;
			delete a.source;
			a.ref =
				type === 'playPlaylist'
					? show.playlists[0]?.id
					: type === 'playSequence'
						? show.sequences[0]?.id
						: type === 'effect'
							? show.effects[0]?.id
							: undefined;
		}
		onchange();
	}

	function refs(a: TriggerAction) {
		if (a.type === 'playPlaylist') return show.playlists;
		if (a.type === 'playSequence' || (a.type === 'surprise' && a.source === 'sequence'))
			return show.sequences;
		if (a.type === 'effect' || a.type === 'surprise') return show.effects;
		return [];
	}

	function targetMode(tg: Target | undefined): 'all' | 'some' {
		return !tg || tg.all ? 'all' : 'some';
	}
	function togglePick(a: TriggerAction, kind: 'propIds' | 'groupIds', id: string) {
		a.target ??= { all: false, propIds: [], groupIds: [] };
		const list = new Set(a.target[kind] ?? []);
		if (list.has(id)) list.delete(id);
		else list.add(id);
		a.target[kind] = [...list];
		onchange();
	}

	function setWindow(t: Trigger, on: boolean) {
		t.activeWindow = on
			? {
					from: { kind: 'sunset', offsetMin: 0 } as TimeSpec,
					to: { kind: 'clock', time: '22:00' } as TimeSpec
				}
			: undefined;
		onchange();
	}

	function summary(t: Trigger): string {
		const bits: string[] = [];
		if (t.cooldownS)
			bits.push(`once per ${t.cooldownS >= 60 ? `${Math.round(t.cooldownS / 60)} min` : `${t.cooldownS} s`}`);
		if (t.maxPerHour) bits.push(`max ${t.maxPerHour}/hour`);
		if (t.when && t.when !== 'always')
			bits.push(WHEN.find((w) => w.value === t.when)?.label.toLowerCase() ?? '');
		if (t.activeWindow) bits.push('in a time window');
		return bits.length ? bits.join(' · ') : 'No limits';
	}

	async function test(t: Trigger) {
		try {
			const r =
				t.action.type === 'surprise'
					? await sensorsApi.testSurprise($state.snapshot(t.action) as TriggerAction)
					: await sensorsApi.fireTrigger(t.id);
			const msg = (r as { message?: string })?.message;
			toasts.success(msg ?? 'Done');
		} catch (e) {
			toasts.error("Couldn't run it", (e as Error).message);
		}
	}
</script>

<div class="te">
	{#each triggers as t (t.id)}
		<article class="trig">
			<div class="row wrap line">
				<span class="kicon" aria-hidden="true">
					{#if t.kind === 'sensor'}<Radar size={16} />{:else if t.kind === 'http'}<Link2
							size={16}
						/>{:else}<CircuitBoard size={16} />{/if}
				</span>
				<input
					class="input grow name"
					bind:value={t.name}
					oninput={onchange}
					aria-label="Trigger name"
					maxlength="60"
				/>
				<button class="btn sm ghost" onclick={() => test(t)} title="Run it now"
					><Play size={14} /> Test</button
				>
				<button class="btn sm ghost icon" onclick={() => remove(t)} aria-label="Remove trigger"
					><Trash2 size={14} /></button
				>
			</div>

			<div class="grid-when">
				<span class="lbl small muted">When</span>
				<div class="row wrap">
					<select
						class="select sm"
						style="width:auto"
						bind:value={t.kind}
						{onchange}
						aria-label="Trigger kind"
					>
						{#if sensorsOn || t.kind === 'sensor'}<option value="sensor"
								>A sensor in the yard{sensorsOn ? '' : ' (turned off)'}</option
							>{/if}
						<option value="gpio">A button wired to the controller</option>
						<option value="http">A link (web request)</option>
					</select>
					{#if t.kind === 'gpio'}
						<span class="small muted">on pin</span>
						<input
							class="input sm num"
							style="width:70px"
							type="number"
							min="2"
							max="27"
							bind:value={t.gpio}
							oninput={onchange}
							aria-label="GPIO pin"
						/>
					{:else if t.kind === 'sensor'}
						{#if sensorInputs.length}
							<select
								class="select sm"
								style="width:auto;max-width:280px"
								value={t.sensor ? `${t.sensor.sensorNodeId}/${t.sensor.input}` : ''}
								onchange={(e) => {
									const [sensorNodeId, input] = e.currentTarget.value.split('/');
									t.sensor = { sensorNodeId, input };
									onchange();
								}}
								aria-label="Sensor input"
							>
								{#if !t.sensor}<option value="">Choose a sensor…</option>{/if}
								{#each sensorInputs as s (s.key)}<option value={s.key}>{s.node.name} · {s.input.name}</option
									>{/each}
							</select>
						{:else}
							<a class="small" href="/settings/sensors">Add a sensor first</a>
						{/if}
					{/if}
				</div>

				<span class="lbl small muted">Do</span>
				<div class="row wrap">
					<select
						class="select sm"
						style="width:auto"
						value={t.action.type}
						onchange={(e) => setType(t.action, e.currentTarget.value as TriggerAction['type'])}
						aria-label="Action"
					>
						{#if surprisesOn || t.action.type === 'surprise'}<option value="surprise"
								>A surprise over the song{surprisesOn ? '' : ' (turned off)'}</option
							>{/if}
						<option value="playPlaylist">Play a playlist</option>
						<option value="playSequence">Play a sequence</option>
						<option value="effect">Show a look</option>
						<option value="stop">Stop the show</option>
					</select>
					{#if t.action.type === 'surprise'}
						<select
							class="select sm"
							style="width:auto"
							value={t.action.source ?? 'effect'}
							onchange={(e) => {
								t.action.source = e.currentTarget.value as 'sequence' | 'effect';
								t.action.ref = refs(t.action)[0]?.id;
								onchange();
							}}
							aria-label="Surprise source"
						>
							<option value="effect">a look</option>
							<option value="sequence">a sequence</option>
						</select>
					{/if}
					{#if t.action.type !== 'stop'}
						<select
							class="select sm"
							style="width:auto;max-width:240px"
							bind:value={t.action.ref}
							{onchange}
							aria-label="What"
						>
							{#each refs(t.action) as o (o.id)}<option value={o.id}>{o.name}</option>{/each}
						</select>
					{/if}
					{#if t.action.type === 'surprise'}
						<span class="small muted">for</span>
						<input
							class="input sm num"
							style="width:64px"
							type="number"
							min="1"
							max="120"
							value={Math.round((t.action.durationMs ?? 5000) / 1000)}
							oninput={(e) => {
								t.action.durationMs = Math.max(1, Number(e.currentTarget.value) || 5) * 1000;
								onchange();
							}}
							aria-label="Seconds"
						/>
						<span class="small muted">s on</span>
						<select
							class="select sm"
							style="width:auto"
							value={targetMode(t.action.target)}
							onchange={(e) => {
								const all = e.currentTarget.value === 'all';
								t.action.target = {
									all,
									propIds: t.action.target?.propIds ?? [],
									groupIds: t.action.target?.groupIds ?? []
								};
								onchange();
							}}
							aria-label="On which props"
						>
							<option value="all">all props</option>
							<option value="some">chosen props…</option>
						</select>
					{/if}
				</div>
			</div>

			{#if t.action.type === 'surprise' && targetMode(t.action.target) === 'some'}
				<div class="picks">
					{#each show.propGroups as g (g.id)}
						<label class="pick small"
							><input
								type="checkbox"
								checked={t.action.target?.groupIds?.includes(g.id)}
								onchange={() => togglePick(t.action, 'groupIds', g.id)}
							/>
							<b>{g.name}</b></label
						>
					{/each}
					{#each show.props as p (p.id)}
						<label class="pick small"
							><input
								type="checkbox"
								checked={t.action.target?.propIds?.includes(p.id)}
								onchange={() => togglePick(t.action, 'propIds', p.id)}
							/>
							{p.name}</label
						>
					{/each}
				</div>
			{/if}

			<button
				class="limits-toggle small"
				onclick={() => (open[t.id] = !open[t.id])}
				aria-expanded={!!open[t.id]}
			>
				<ChevronDown size={14} class={open[t.id] ? 'rot' : ''} /> Limits:
				<span class="muted">{summary(t)}</span>
			</button>
			{#if open[t.id]}
				<div class="limits">
					<label class="fld small"
						><span class="muted">Wait between runs</span>
						<span class="row"
							><input
								class="input sm num"
								style="width:80px"
								type="number"
								min="0"
								max="86400"
								value={t.cooldownS ?? 0}
								oninput={(e) => {
									t.cooldownS = Math.max(0, Number(e.currentTarget.value) || 0) || undefined;
									onchange();
								}}
								aria-label="Cooldown seconds"
							/> s</span
						></label
					>
					<label class="fld small"
						><span class="muted">At most per hour (0 = no limit)</span>
						<input
							class="input sm num"
							style="width:80px"
							type="number"
							min="0"
							max="3600"
							value={t.maxPerHour ?? 0}
							oninput={(e) => {
								t.maxPerHour = Math.max(0, Number(e.currentTarget.value) || 0) || undefined;
								onchange();
							}}
							aria-label="Maximum per hour"
						/></label
					>
					<label class="fld small"
						><span class="muted">When</span>
						<select
							class="select sm"
							value={t.when ?? 'always'}
							onchange={(e) => {
								const v = e.currentTarget.value as TriggerWhen;
								t.when = v === 'always' ? undefined : v;
								onchange();
							}}
							aria-label="When it may run"
						>
							{#each WHEN as w (w.value)}<option value={w.value}>{w.label}</option>{/each}
						</select></label
					>
					<div class="fld small wide">
						<label class="row"
							><input
								type="checkbox"
								checked={!!t.activeWindow}
								onchange={(e) => setWindow(t, e.currentTarget.checked)}
							/>
							<span>Only between certain times</span></label
						>
						{#if t.activeWindow}
							<div class="row wrap win">
								<TimeSpecPicker
									bind:value={t.activeWindow.from}
									location={show.schedule.location}
									label="From"
								/>
								<span class="muted">to</span>
								<TimeSpecPicker bind:value={t.activeWindow.to} location={show.schedule.location} label="To" />
							</div>
						{/if}
					</div>
				</div>
			{/if}
			{#if t.kind === 'http'}<code class="mono faint tiny">POST {location.origin}/api/v1/triggers/{t.id}</code
				>{/if}
		</article>
	{:else}
		<div class="faint small empty">No triggers yet.</div>
	{/each}

	<div class="row wrap adds">
		{#if sensorsOn && surprisesOn}<button class="btn sm" onclick={() => add('sensor')}
				><Sparkles size={14} /> Sensor surprise</button
			>{/if}
		<button class="btn sm ghost" onclick={() => add('gpio')}
			><Plus size={14} /> Button on the controller</button
		>
		<button class="btn sm ghost" onclick={() => add('http')}><Zap size={14} /> Web link</button>
	</div>
</div>

<style>
	.te {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.trig {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 14px;
		border: 1px solid var(--border-2);
		border-radius: var(--r-3);
		background: var(--surface-2);
	}
	.line {
		gap: 8px;
		align-items: center;
	}
	.kicon {
		display: inline-flex;
		color: var(--accent-text);
	}
	.name {
		min-width: 160px;
	}
	.grid-when {
		display: grid;
		grid-template-columns: 44px minmax(0, 1fr);
		gap: 8px 10px;
		align-items: center;
	}
	.grid-when .row {
		gap: 6px;
		align-items: center;
	}
	.picks {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
		gap: 4px 12px;
		max-height: 180px;
		overflow: auto;
		padding: 8px;
		border-radius: var(--r-2);
		background: var(--surface);
	}
	.pick {
		display: flex;
		gap: 6px;
		align-items: center;
	}
	.limits-toggle {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		align-self: flex-start;
		background: none;
		border: 0;
		padding: 0;
		color: var(--text-2);
		cursor: pointer;
	}
	.limits-toggle :global(.rot) {
		transform: rotate(180deg);
	}
	.limits {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		gap: 12px 16px;
	}
	.fld {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.fld .row {
		gap: 6px;
		align-items: center;
	}
	.wide {
		grid-column: 1 / -1;
	}
	.win {
		gap: 8px;
		align-items: flex-start;
		margin-top: 6px;
	}
	.adds {
		gap: 8px;
	}
	.empty {
		padding: 8px 0;
	}
	@media (max-width: 600px) {
		.grid-when {
			grid-template-columns: 1fr;
		}
	}
</style>

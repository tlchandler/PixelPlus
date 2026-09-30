<script lang="ts">
	import type { EffectParams, ParamSpec } from '$lib/api/types';
	import Switch from '$lib/components/ui/Switch.svelte';
	import { Plus, X } from '@lucide/svelte';

	let {
		schema,
		params = $bindable(),
		onchange
	}: { schema: ParamSpec[]; params: EffectParams; onchange?: () => void } = $props();

	function set(k: string, v: EffectParams[string]) {
		params = { ...params, [k]: v };
		onchange?.();
	}
	function val<T>(s: ParamSpec): T {
		return (params[s.key] ?? s.default) as T;
	}
	const pretty = (o: string) => o.charAt(0).toUpperCase() + o.slice(1);
	const presets = ['#ff0000', '#00c000', '#ffffff', '#ffb46b', '#0040ff', '#ffd700', '#a020f0', '#00e5ff'];
</script>

<div class="params">
	{#each schema as s (s.key)}
		<div class="p">
			<div class="row between">
				<span class="label">{s.label}</span>
				{#if s.kind === 'number'}<span class="num v">{val<number>(s)}{s.unit ? ` ${s.unit}` : ''}</span>{/if}
			</div>
			{#if s.kind === 'number'}
				{@const v = val<number>(s)}
				{@const min = s.min ?? 0}
				{@const max = s.max ?? 1}
				<input
					type="range"
					class="range"
					{min}
					{max}
					step={s.step ?? 0.01}
					value={v}
					style:--pct="{((v - min) / (max - min || 1)) * 100}%"
					oninput={(e) => set(s.key, Number((e.target as HTMLInputElement).value))}
					aria-label={s.label}
				/>
			{:else if s.kind === 'bool'}
				<Switch checked={val<boolean>(s)} label={s.label} onchange={(v) => set(s.key, v)} />
			{:else if s.kind === 'color'}
				<div class="row">
					<input
						type="color"
						value={val<string>(s)}
						oninput={(e) => set(s.key, (e.target as HTMLInputElement).value)}
						aria-label={s.label}
					/>
					<span class="mono faint">{val<string>(s)}</span>
					<span class="grow"></span>
					{#each presets.slice(0, 5) as c (c)}<button
							type="button"
							class="dot-sw"
							style:background={c}
							onclick={() => set(s.key, c)}
							aria-label="Use {c}"
						></button>{/each}
				</div>
			{:else if s.kind === 'colors'}
				{@const list = val<string[]>(s)}
				<div class="row wrap">
					{#each list as c, i (i)}
						<span class="csw">
							<input
								type="color"
								value={c}
								oninput={(e) =>
									set(
										s.key,
										list.map((x, k) => (k === i ? (e.target as HTMLInputElement).value : x))
									)}
								aria-label="{s.label} {i + 1}"
							/>
							{#if list.length > 1}<button
									type="button"
									class="rm"
									onclick={() =>
										set(
											s.key,
											list.filter((_, k) => k !== i)
										)}
									aria-label="Remove color {i + 1}"><X size={10} /></button
								>{/if}
						</span>
					{/each}
					{#if list.length < 16}
						<button
							type="button"
							class="addc"
							onclick={() => set(s.key, [...list, presets[list.length % presets.length]])}
							aria-label="Add color"><Plus size={14} /></button
						>
					{/if}
				</div>
			{:else if s.kind === 'select'}
				{@const opts = s.options ?? []}
				{#if opts.length <= 4}
					<div class="seg">
						{#each opts as o (o)}<button
								type="button"
								class:on={val<string>(s) === o}
								aria-pressed={val<string>(s) === o}
								onclick={() => set(s.key, o)}>{pretty(o)}</button
							>{/each}
					</div>
				{:else}
					<select
						class="select sm"
						value={val<string>(s)}
						onchange={(e) => set(s.key, (e.target as HTMLSelectElement).value)}
						aria-label={s.label}
					>
						{#each opts as o (o)}<option value={o}>{pretty(o)}</option>{/each}
					</select>
				{/if}
			{/if}
			{#if s.help}<span class="hint">{s.help}</span>{/if}
		</div>
	{/each}
</div>

<style>
	.params {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.p {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.label {
		font-size: 12.5px;
		font-weight: 560;
		color: var(--text-2);
	}
	.v {
		font-size: 12px;
		color: var(--text-3);
	}
	.hint {
		font-size: 11.5px;
		color: var(--text-3);
	}
	.dot-sw {
		width: 20px;
		height: 20px;
		border-radius: 50%;
		box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.2);
	}
	.csw {
		position: relative;
	}
	.rm {
		position: absolute;
		top: -6px;
		right: -6px;
		width: 16px;
		height: 16px;
		border-radius: 50%;
		display: none;
		place-items: center;
		background: var(--surface-3);
		color: var(--text);
		border: 1px solid var(--border-2);
	}
	.csw:hover .rm,
	.csw:focus-within .rm {
		display: grid;
	}
	@media (pointer: coarse) {
		.rm {
			display: grid;
		}
	}
	.addc {
		width: 32px;
		height: 32px;
		border-radius: 8px;
		border: 1.5px dashed var(--border-3);
		display: grid;
		place-items: center;
		color: var(--text-3);
	}
	.addc:hover {
		border-color: var(--accent);
		color: var(--accent);
	}
	.seg {
		display: flex;
		gap: 2px;
		padding: 3px;
		border-radius: 9px;
		background: var(--surface-2);
		border: 1px solid var(--border);
	}
	.seg button {
		flex: 1;
		height: 28px;
		border-radius: 6px;
		font-size: 12px;
		font-weight: 540;
		color: var(--text-2);
	}
	.seg button.on {
		background: var(--surface-3);
		color: var(--text);
		box-shadow: 0 0 0 1px var(--border-2);
	}
</style>

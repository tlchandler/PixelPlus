<script lang="ts">
	let {
		checked = $bindable(false),
		label,
		disabled = false,
		onchange,
		size = 'md'
	}: {
		checked?: boolean;
		label: string;
		disabled?: boolean;
		onchange?: (v: boolean) => void;
		size?: 'sm' | 'md';
	} = $props();
</script>

<button
	type="button"
	role="switch"
	class="switch {size}"
	aria-checked={checked}
	aria-label={label}
	{disabled}
	onclick={() => {
		checked = !checked;
		onchange?.(checked);
	}}
>
	<span class="knob"></span>
</button>

<style>
	.switch {
		--w: 40px;
		--h: 24px;
		position: relative;
		width: var(--w);
		height: var(--h);
		border-radius: 99px;
		background: var(--surface-3);
		border: 1px solid var(--border-2);
		transition:
			background 200ms var(--ease),
			border-color 200ms var(--ease);
		flex: 0 0 auto;
	}
	.switch.sm {
		--w: 34px;
		--h: 20px;
	}
	.switch::before {
		content: '';
		position: absolute;
		inset: -10px -4px;
	}
	.knob {
		position: absolute;
		top: 2px;
		left: 2px;
		width: calc(var(--h) - 6px);
		height: calc(var(--h) - 6px);
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
		transition: transform 220ms var(--ease-spring);
	}
	.switch[aria-checked='true'] {
		background: var(--accent);
		border-color: transparent;
	}
	.switch[aria-checked='true'] .knob {
		transform: translateX(calc(var(--w) - var(--h)));
	}
	.switch:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
</style>

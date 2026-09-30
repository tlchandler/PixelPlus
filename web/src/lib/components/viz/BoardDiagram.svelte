<script lang="ts">
	import type { BoardKind } from '$lib/api/types';

	let {
		board,
		rev,
		portPixels = [],
		receivers = {},
		selectedJack = null,
		onjack,
		compact = false,
		warnPort3 = false
	}: {
		board: BoardKind;
		rev?: string;
		/** pixels per output (index = output-1) */
		portPixels?: number[];
		/** jack -> receiver name */
		receivers?: Record<number, string>;
		selectedJack?: number | null;
		onjack?: (jack: number) => void;
		compact?: boolean;
		warnPort3?: boolean;
	} = $props();

	const px = (o: number) => portPixels[o - 1] ?? 0;
	const jackUsed = (j: number) => [1, 2, 3, 4].some((p) => px((j - 1) * 4 + p) > 0);
	const over = (o: number) => px(o) > 1600;
	const uid = Math.random().toString(36).slice(2, 8);
</script>

{#snippet rj45(x: number, y: number, jack: number, w = 56, label = `J${jack}`)}
	{@const used = jackUsed(jack)}
	{@const sel = selectedJack === jack}
	<g
		class="jack"
		class:used
		class:sel
		class:clickable={!!onjack}
		transform="translate({x} {y})"
		role={onjack ? 'button' : undefined}
		tabindex={onjack ? 0 : undefined}
		aria-label={onjack ? `Jack ${jack}${receivers[jack] ? `, ${receivers[jack]} receiver` : ''}` : undefined}
		onclick={() => onjack?.(jack)}
		onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), onjack?.(jack))}
	>
		<rect class="hit" x="-6" y="-26" width={w + 12} height="96" rx="8" />
		{#each [1, 2, 3, 4] as p (p)}
			{@const o = (jack - 1) * 4 + p}
			<circle
				cx={w - 8 - (p - 1) * ((w - 16) / 3)}
				cy="-12"
				r="4"
				class="led"
				class:on={px(o) > 0}
				class:bad={over(o)}
				class:warn={warnPort3 && p === 3}
			/>
		{/each}
		<rect class="body" width={w} height="44" rx="5" />
		<rect class="mouth" x={w * 0.18} y="10" width={w * 0.64} height="26" rx="2" />
		<rect class="clip" x={w * 0.36} y="30" width={w * 0.28} height="8" rx="1" />
		{#if !compact}
			<text class="lbl" x={w / 2} y="62" text-anchor="middle">{label}</text>
		{/if}
	</g>
{/snippet}

{#if board === 'difftxlarge'}
	<svg viewBox="0 0 1300 560" class="board" role="img" aria-label="60-port transmitter board diagram with 15 jacks">
		<defs>
			<linearGradient id="pcb-{uid}" x1="0" y1="0" x2="0" y2="1">
				<stop offset="0" stop-color="#153524" />
				<stop offset="1" stop-color="#0e2519" />
			</linearGradient>
		</defs>
		<rect x="4" y="4" width="1292" height="552" rx="18" fill="url(#pcb-{uid})" stroke="#2c5a40" stroke-width="2" />
		{#each [[24, 24], [1276, 24], [24, 536], [1276, 536], [650, 536]] as [cx, cy] (cx + '-' + cy)}
			<circle {cx} {cy} r="9" fill="#0a1a11" stroke="#3d7355" stroke-width="2" />
		{/each}
		<!-- traces -->
		<g stroke="#1f4a33" stroke-width="3" fill="none" opacity=".8">
			{#each Array(15) as _, i (i)}
				<path d="M{120 + i * 80} 330 C {120 + i * 80} 300, {330 + i * 12} 300, {330 + i * 12} 250" />
			{/each}
		</g>
		<!-- Pi footprint -->
		<rect x="50" y="46" width="340" height="190" rx="10" fill="none" stroke="#6fa587" stroke-dasharray="6 6" opacity=".6" />
		<text x="220" y="136" class="silk" text-anchor="middle">RASPBERRY PI 3B+ / 4 / 5</text>
		<text x="220" y="158" class="silk small" text-anchor="middle">face up on standoffs</text>
		<rect x="450" y="210" width="210" height="28" rx="3" fill="#0b0b0d" stroke="#333" />
		{#each Array(20) as _, i (i)}<circle cx={462 + i * 9.6} cy="224" r="2.4" fill="#c9a24a" />{/each}
		<!-- chips -->
		{#each [720, 800, 880] as x (x)}<rect {x} y="90" width="48" height="36" rx="3" fill="#111" /><text x={x + 24} y="146" class="silk small" text-anchor="middle">LATCH</text>{/each}
		<!-- OLED -->
		<rect x="1040" y="120" width="130" height="80" rx="6" fill="#050608" stroke="#3a3f46" />
		<rect x="1050" y="130" width="110" height="52" rx="2" fill="#0b1a2a" />
		<text x="1105" y="162" class="oled" text-anchor="middle">PIXELPLUS</text>
		<text x="1105" y="218" class="silk small" text-anchor="middle">OLED</text>
		<!-- power -->
		<rect x="1150" y="30" width="96" height="44" rx="4" fill="#1b4fd6" />
		<circle cx="1176" cy="52" r="10" fill="#c7ccd6" /><circle cx="1220" cy="52" r="10" fill="#c7ccd6" />
		<text x="1198" y="94" class="silk" text-anchor="middle">12V IN</text>
		<rect x="940" y="40" width="70" height="26" rx="3" fill="#2a2a2a" /><text x="975" y="84" class="silk small" text-anchor="middle">FUSE 5A</text>
		<circle cx="880" cy="200" r="22" fill="#9aa3ad" stroke="#666" /><text x="880" y="240" class="silk small" text-anchor="middle">RTC</text>
		<text x="760" y="215" class="silk title" text-anchor="middle">difftxlarge{rev ? ` rev ${rev}` : ''}</text>
		<text x="760" y="238" class="silk small" text-anchor="middle">60 outputs · 15 × RJ45 differential</text>
		<!-- jacks -->
		{#each Array(15) as _, i (i)}
			{@render rj45(64 + i * 80, 380, i + 1)}
			{#if receivers[i + 1] && !compact}
				<text x={92 + i * 80} y="468" class="rx" text-anchor="middle">{receivers[i + 1].length > 9 ? receivers[i + 1].slice(0, 8) + '…' : receivers[i + 1]}</text>
			{/if}
		{/each}
		<text x="650" y="516" class="silk small" text-anchor="middle">BANK 1: J1–J5 · BANK 2: J6–J10 · BANK 3: J11–J15</text>
	</svg>
{:else if board === 'difftx'}
	<svg viewBox="0 0 520 250" class="board" role="img" aria-label="PixelPlus pHAT board diagram">
		<defs>
			<linearGradient id="pcb2-{uid}" x1="0" y1="0" x2="0" y2="1">
				<stop offset="0" stop-color="#153524" />
				<stop offset="1" stop-color="#0e2519" />
			</linearGradient>
		</defs>
		<rect x="4" y="4" width="512" height="236" rx="26" fill="url(#pcb2-{uid})" stroke="#2c5a40" stroke-width="2" />
		{#each [[34, 34], [486, 34], [34, 210], [486, 210]] as [cx, cy] (cx + '-' + cy)}
			<circle {cx} {cy} r="13" fill="#0a1a11" stroke="#c9a24a" stroke-width="3" />
		{/each}
		<rect x="72" y="18" width="376" height="36" rx="3" fill="#0b0b0d" />
		{#each Array(20) as _, i (i)}
			<circle cx={84 + i * 18.6} cy="28" r="3" fill="#c9a24a" /><circle cx={84 + i * 18.6} cy="44" r="3" fill="#c9a24a" />
		{/each}
		<rect x="60" y="150" width="84" height="56" rx="4" fill="#1b4fd6" />
		<circle cx="82" cy="178" r="11" fill="#c7ccd6" /><circle cx="122" cy="178" r="11" fill="#c7ccd6" />
		<text x="102" y="226" class="silk small" text-anchor="middle">5V IN</text>
		<rect x="180" y="90" width="80" height="60" rx="4" fill="#161616" />
		<rect x="280" y="86" width="40" height="80" rx="3" fill="#161616" />
		<text x="200" y="80" class="silk small">FPP RS-422 pHAT{rev ? ` rev ${rev}` : ''}</text>
		{@render rj45(360, 110, 1, 110, 'Ports 1–4')}
	</svg>
{:else if board === 'diffsmart'}
	<svg viewBox="0 0 560 300" class="board" role="img" aria-label="Smart receiver board diagram">
		<defs>
			<linearGradient id="pcb3-{uid}" x1="0" y1="0" x2="0" y2="1">
				<stop offset="0" stop-color="#153524" />
				<stop offset="1" stop-color="#0e2519" />
			</linearGradient>
		</defs>
		<rect x="4" y="4" width="552" height="292" rx="16" fill="url(#pcb3-{uid})" stroke="#2c5a40" stroke-width="2" />
		<rect x="40" y="30" width="200" height="120" rx="8" fill="none" stroke="#6fa587" stroke-dasharray="6 6" opacity=".6" />
		<text x="140" y="95" class="silk" text-anchor="middle">PI ZERO 2 W</text>
		<rect x="290" y="40" width="70" height="40" rx="4" fill="#1a1a1a" stroke="#555" />
		<rect x="296" y="46" width="28" height="28" rx="3" fill="#F5A524" />
		<text x="325" y="100" class="silk small" text-anchor="middle">SW1 · PI / RX</text>
		<rect x="400" y="30" width="120" height="60" rx="4" fill="#1b4fd6" />
		<text x="460" y="112" class="silk small" text-anchor="middle">12V IN</text>
		{#each [1, 2, 3, 4] as o (o)}
			<g transform="translate({60 + (o - 1) * 118} 190)">
				<rect width="96" height="50" rx="4" fill={px(o) > 0 ? '#F5A524' : '#2f7a4a'} opacity={px(o) > 0 ? 1 : 0.85} />
				{#each [0, 1, 2] as k (k)}<circle cx={20 + k * 28} cy="25" r="9" fill="#c7ccd6" />{/each}
				<text x="48" y="72" class="lbl" text-anchor="middle">Out {o}</text>
			</g>
		{/each}
	</svg>
{:else}
	<svg viewBox="0 0 400 240" class="board" role="img" aria-label="Raspberry Pi">
		<rect x="40" y="30" width="320" height="180" rx="16" fill="#153524" stroke="#2c5a40" stroke-width="2" />
		<rect x="70" y="50" width="240" height="22" rx="3" fill="#0b0b0d" />
		<rect x="170" y="110" width="60" height="60" rx="4" fill="#191919" />
		<text x="200" y="198" class="silk" text-anchor="middle">{board === 'virtual' ? 'VIRTUAL LEADER' : 'RASPBERRY PI'}</text>
	</svg>
{/if}

<style>
	.board {
		width: 100%;
		height: auto;
		display: block;
		filter: drop-shadow(0 12px 30px rgba(0, 0, 0, 0.35));
	}
	.silk {
		fill: #d8e8dd;
		font: 600 15px var(--font);
		letter-spacing: 0.04em;
		opacity: 0.85;
	}
	.silk.small {
		font-size: 12px;
		font-weight: 500;
		opacity: 0.65;
	}
	.silk.title {
		font-size: 22px;
		letter-spacing: 0;
	}
	.oled {
		fill: #7fd3ff;
		font: 700 14px var(--mono);
	}
	.lbl {
		fill: #e9f2ec;
		font: 700 15px var(--font);
	}
	.rx {
		fill: var(--accent);
		font: 600 11.5px var(--font);
	}
	.jack .body {
		fill: #3a3d43;
		stroke: #555a62;
		stroke-width: 1.5;
		transition: fill 160ms;
	}
	.jack .mouth {
		fill: #16181b;
	}
	.jack .clip {
		fill: #2a2c30;
	}
	.jack.used .body {
		fill: #6b5a2d;
		stroke: var(--accent);
	}
	.jack.sel .body {
		stroke: #fff;
		stroke-width: 3;
	}
	.jack .hit {
		fill: transparent;
	}
	.jack.clickable {
		cursor: pointer;
	}
	.jack.clickable:hover .hit {
		fill: rgba(255, 255, 255, 0.06);
	}
	.jack.clickable:focus-visible {
		outline: none;
	}
	.jack.clickable:focus-visible .hit {
		stroke: var(--accent);
		stroke-width: 2;
	}
	.led {
		fill: #26302a;
		stroke: #0c140f;
	}
	.led.on {
		fill: #56f39a;
		filter: drop-shadow(0 0 4px #56f39a);
	}
	.led.bad {
		fill: #ff5a5a;
		filter: drop-shadow(0 0 4px #ff5a5a);
	}
	.led.warn {
		stroke: #F5A524;
		stroke-width: 2;
	}
</style>

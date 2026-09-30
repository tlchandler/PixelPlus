<script lang="ts">
	import qrcode from 'qrcode-generator';

	let {
		text,
		size = 160,
		fg = '#000',
		bg = '#fff'
	}: { text: string; size?: number; fg?: string; bg?: string } = $props();

	const model = $derived.by(() => {
		const q = qrcode(0, 'M');
		q.addData(text || ' ');
		q.make();
		const n = q.getModuleCount();
		let d = '';
		for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) if (q.isDark(y, x)) d += `M${x} ${y}h1v1h-1z`;
		return { n, d };
	});
</script>

<svg
	width={size}
	height={size}
	viewBox="-2 -2 {model.n + 4} {model.n + 4}"
	role="img"
	aria-label="QR code for {text}"
	shape-rendering="crispEdges"
>
	<rect x="-2" y="-2" width={model.n + 4} height={model.n + 4} fill={bg} rx="1.5" />
	<path d={model.d} fill={fg} />
</svg>

import type { BoardKind, Node, Prop, Receiver, ReceiverKind, Show } from '$lib/api/types';

export interface BoardInfo {
	id: BoardKind;
	name: string;
	short: string;
	outputs: number;
	jacks: number;
	blurb: string;
}

export const BOARDS: Record<BoardKind, BoardInfo> = {
	difftxlarge: {
		id: 'difftxlarge',
		name: '60-Port Transmitter',
		short: 'difftxlarge',
		outputs: 60,
		jacks: 15,
		blurb: '15 network jacks, 60 pixel outputs, power monitor, clock and temperature sensors.'
	},
	difftx: {
		id: 'difftx',
		name: 'PixelPlus pHAT',
		short: 'difftx',
		outputs: 4,
		jacks: 1,
		blurb: 'Compact 4-output transmitter that sits on a Raspberry Pi Zero.'
	},
	diffsmart: {
		id: 'diffsmart',
		name: 'Smart Receiver (standalone)',
		short: 'diffsmart',
		outputs: 4,
		jacks: 0,
		blurb: 'Four screw-terminal outputs driven directly by the Pi on the board.'
	},
	'bare-pi': {
		id: 'bare-pi',
		name: 'Raspberry Pi (no board)',
		short: 'Raspberry Pi',
		outputs: 0,
		jacks: 0,
		blurb: 'Show director and audio only — no pixel outputs.'
	},
	virtual: {
		id: 'virtual',
		name: 'Virtual (Docker / PC)',
		short: 'Virtual',
		outputs: 0,
		jacks: 0,
		blurb: 'Runs the show from a PC or NAS and drives followers over the network.'
	}
};

export const RECEIVERS: Record<ReceiverKind, { name: string; short: string; ports: number; fuse?: number }> =
	{
		diffrx: {
			name: 'Chandler 4D/8P Differential Receiver',
			short: 'Differential receiver',
			ports: 4,
			fuse: 6
		},
		'diffsmart-rx': { name: 'Chandler Smart Receiver (RX mode)', short: 'Smart receiver', ports: 4, fuse: 6 },
		'generic-4': { name: 'Generic 4-port receiver (Falcon/Kulp style)', short: '4-port receiver', ports: 4 },
		direct: { name: 'Direct connection', short: 'Direct', ports: 1 }
	};

/** Short output id, as printed next to the jacks: "J3-2" on the 60-port board, else "Port 2". */
export function outputLabel(board: BoardKind, index: number): string {
	if (board === 'difftxlarge') {
		const k = index - 1;
		return `J${Math.floor(k / 4) + 1}-${(k % 4) + 1}`;
	}
	return `Port ${index}`;
}

/** The name people use for an output: "J3 · Port 2" on the 60-port board, else "Port 2". */
export function portName(board: BoardKind, index: number): string {
	if (board === 'difftxlarge') return `J${Math.floor((index - 1) / 4) + 1} · Port ${portOf(index)}`;
	return `Port ${index}`;
}

export function jackOf(board: BoardKind, output: number): number | null {
	if (board === 'difftxlarge') return Math.floor((output - 1) / 4) + 1;
	if (board === 'difftx') return 1;
	return null;
}

export function portOf(output: number): number {
	return ((output - 1) % 4) + 1;
}

export function receiverFor(show: Show, nodeId: string, output: number): Receiver | undefined {
	const node = show.nodes.find((n) => n.id === nodeId);
	if (!node) return undefined;
	const jack = jackOf(node.board, output);
	if (jack == null) return undefined;
	return show.receivers.find((r) => r.nodeId === nodeId && r.jack === jack);
}

export interface ChainStep {
	label: string;
	kind: 'node' | 'jack' | 'receiver' | 'port' | 'pixels';
}

/** Human wiring chain, e.g. Main Controller › J3 › Front Yard receiver › Port 2 › pixels 1–150 */
export function wiringChain(show: Show, seg: Prop['segments'][number]): ChainStep[] {
	const node = show.nodes.find((n) => n.id === seg.nodeId);
	const steps: ChainStep[] = [];
	if (!node) return [{ label: 'Unknown controller', kind: 'node' }];
	steps.push({ label: node.name, kind: 'node' });
	const jack = jackOf(node.board, seg.output);
	const rx = receiverFor(show, node.id, seg.output);
	// The jack only adds information in front of a receiver: a bare output is labelled "J3-2" already.
	if (node.board === 'difftxlarge' && jack && rx) steps.push({ label: `J${jack}`, kind: 'jack' });
	if (rx) {
		steps.push({ label: `${rx.name} receiver`.replace(/receiver receiver$/i, 'receiver'), kind: 'receiver' });
		steps.push({ label: `Port ${portOf(seg.output)}`, kind: 'port' });
	} else {
		steps.push({ label: portName(node.board, seg.output), kind: 'port' });
	}
	const a = seg.startPixel + 1;
	const b = seg.startPixel + seg.pixelCount;
	steps.push({ label: `pixels ${a}–${b}`, kind: 'pixels' });
	return steps;
}

export function wiringText(show: Show, prop: Prop): string {
	if (!prop.segments.length) return 'Not wired yet';
	const s = wiringChain(show, prop.segments[0])
		.map((x) => x.label)
		.join(' › ');
	return prop.segments.length > 1 ? `${s} +${prop.segments.length - 1} more` : s;
}

/** Props (in chain order) hanging off one node output. */
export function propsOnOutput(
	show: Show,
	nodeId: string,
	output: number
): { prop: Prop; seg: Prop['segments'][number] }[] {
	const out: { prop: Prop; seg: Prop['segments'][number] }[] = [];
	for (const p of show.props)
		for (const s of p.segments) if (s.nodeId === nodeId && s.output === output) out.push({ prop: p, seg: s });
	return out.sort((a, b) => a.seg.startPixel - b.seg.startPixel);
}

export function pixelsOnOutput(show: Show, nodeId: string, output: number): number {
	return propsOnOutput(show, nodeId, output).reduce(
		(m, x) => Math.max(m, x.seg.startPixel + x.seg.pixelCount),
		0
	);
}

export function nodeUsage(show: Show, node: Node): { used: number; pixels: number } {
	let used = 0;
	let pixels = 0;
	for (let i = 1; i <= node.outputs.length; i++) {
		const px = pixelsOnOutput(show, node.id, i);
		if (px) used++;
		pixels += px;
	}
	return { used, pixels };
}

/** difftx rev D has port 3 wired backwards (needs a 4/5 swapped lead). */
export function needsPort3Warning(node: Node): boolean {
	return node.board === 'difftx' && (node.boardRev ?? '').toUpperCase() === 'D';
}

/** Re-pack the chain on one output after a reorder: consecutive startPixel respecting nulls. */
export function repackChain(
	order: { propId: string; segIndex: number; pixelCount: number; nullPixels: number }[]
): Map<string, number> {
	const starts = new Map<string, number>();
	let cursor = 0;
	for (const o of order) {
		cursor += o.nullPixels;
		starts.set(`${o.propId}:${o.segIndex}`, cursor);
		cursor += o.pixelCount;
	}
	return starts;
}

export const MAX_PIXELS_PER_OUTPUT = 1600;

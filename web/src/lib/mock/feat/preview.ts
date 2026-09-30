// WS2 (F3): browser preview files for demo mode. Frames are synthesized (a rainbow that
// pulses on a 120 BPM beat) and gzip-compressed per block, exactly like the daemon's PPPV.
import type { Prop } from '$lib/api/types';
import type { FeatureContext } from './context';
import { HttpError } from './context';
import { tempShows } from './autoshow';
import type { PppvHeader, PppvProp } from '$lib/preview/pppv';

const BLOCK = 64;
const MAX_PROP = 300;

function plan(props: Prop[]): PppvProp[] {
	return props.map((p) => {
		const n = Math.min(p.pixelCount, MAX_PROP);
		if (n >= p.pixelCount) return { id: p.id, n: p.pixelCount };
		const idx = Array.from({ length: n }, (_, i) =>
			Math.round((i * (p.pixelCount - 1)) / Math.max(1, n - 1))
		);
		return { id: p.id, n, idx };
	});
}

function hsv(h: number, v: number): [number, number, number] {
	const f = (k: number) => {
		const x = (k + h / 60) % 6;
		return v * (1 - Math.max(0, Math.min(x, 4 - x, 1)));
	};
	return [Math.round(f(5) * 255), Math.round(f(3) * 255), Math.round(f(1) * 255)];
}

function header(seqId: string, durationMs: number, frameMs: number, props: Prop[]): PppvHeader {
	const fm = Math.max(50, frameMs);
	const frameCount = Math.max(1, Math.ceil(durationMs / fm));
	const pl = plan(props);
	const frameBytes = pl.reduce((a, p) => a + p.n * 3, 0);
	const blocks = Array.from({ length: Math.ceil(frameCount / BLOCK) }, (_, i) => ({ offset: i, len: 0 }));
	return {
		v: 1,
		seqId,
		frameMs: fm,
		frameCount,
		frameBytes,
		props: pl,
		blockFrames: BLOCK,
		blocks,
		mappingHash: 'demo'
	};
}

async function gzip(raw: Uint8Array): Promise<Blob> {
	const stream = new Blob([raw as BlobPart]).stream().pipeThrough(new CompressionStream('gzip'));
	return new Blob([await new Response(stream).arrayBuffer()], { type: 'application/octet-stream' });
}

function renderBlock(h: PppvHeader, n: number): Uint8Array {
	const first = n * h.blockFrames;
	const count = Math.min(h.blockFrames, h.frameCount - first);
	const out = new Uint8Array(Math.max(0, count) * h.frameBytes);
	let o = 0;
	for (let f = first; f < first + count; f++) {
		const t = f * h.frameMs;
		const pulse = 0.35 + 0.65 * Math.exp(-(t % 500) / 140);
		h.props.forEach((p, pi) => {
			for (let k = 0; k < p.n; k++) {
				const [r, g, b] = hsv(
					((((k / Math.max(1, p.n)) * 360 + t * 0.12 + pi * 47) % 360) + 360) % 360,
					pulse
				);
				out[o++] = r;
				out[o++] = g;
				out[o++] = b;
			}
		});
	}
	return out;
}

export function register(ctx: FeatureContext) {
	const find = (id: string): PppvHeader => {
		const show = ctx.server.show;
		const tmp = tempShows.get(id);
		if (tmp != null) return header(id, tmp, 50, show.props);
		const s = show.sequences.find((x) => x.id === id);
		if (!s) throw new HttpError(404, 'not_found', 'That sequence doesn’t exist.');
		return header(s.id, s.durationMs, s.frameMs, show.props);
	};
	ctx.route('GET', '/sequences/([^/]+)/preview', ({ params }) => find(params[0]));
	ctx.route('GET', '/sequences/([^/]+)/preview/block/(\\d+)', async ({ params }) => {
		const h = find(params[0]);
		const n = Number(params[1]);
		if (!(n >= 0 && n < h.blocks.length)) throw new HttpError(404, 'not_found', 'That part of the preview');
		return gzip(renderBlock(h, n));
	});
	ctx.route('GET', '/sequences/([^/]+)/preview/data', () => {
		throw new HttpError(404, 'not_ready', 'Demo mode serves previews block by block.');
	});
}

// Reader for the `PPPV` v1 sequence preview format (F3, ARCHITECTURE §12.3).
//
//   "PPPV" | u32 LE header length | header JSON | gzip block 0 | gzip block 1 | …
//
// A frame holds, per prop in header order, `n` RGB triplets. Blocks of `blockFrames`
// frames are gzip compressed on their own and fetched lazily (one request per block),
// then inflated with the browser's native DecompressionStream.
import type { PropSlot } from './source';

export interface PppvProp {
	id: string;
	n: number;
	/** Prop pixel index of each sample; absent = all pixels in order. */
	idx?: number[];
}

export interface PppvHeader {
	v: number;
	seqId: string;
	frameMs: number;
	frameCount: number;
	frameBytes: number;
	props: PppvProp[];
	blockFrames: number;
	blocks: { offset: number; len: number }[];
	mappingHash: string;
}

const MAGIC = [0x50, 0x50, 0x50, 0x56]; // "PPPV"

/** Split a whole `.pppv` file into its header and the raw file bytes (blocks are at `offset`). */
export function parsePppv(buf: ArrayBuffer): { header: PppvHeader; bytes: Uint8Array } {
	const bytes = new Uint8Array(buf);
	if (bytes.length < 8 || MAGIC.some((m, i) => bytes[i] !== m)) throw new Error('Not a preview file');
	const len = new DataView(buf).getUint32(4, true);
	if (8 + len > bytes.length) throw new Error('The preview file is cut short');
	const header = JSON.parse(new TextDecoder().decode(bytes.subarray(8, 8 + len))) as PppvHeader;
	if (header.v !== 1) throw new Error(`Unsupported preview version ${header.v}`);
	return { header, bytes };
}

/** Inflate one gzip block. */
export async function inflate(gz: Uint8Array): Promise<Uint8Array> {
	const stream = new Blob([gz as BlobPart]).stream().pipeThrough(new DecompressionStream('gzip'));
	return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** Frame shown at `ms` (clamped to the sequence). */
export function frameIndex(h: Pick<PppvHeader, 'frameMs' | 'frameCount'>, ms: number): number {
	if (!(ms > 0) || !h.frameCount) return 0;
	return Math.min(h.frameCount - 1, Math.floor(ms / Math.max(1, h.frameMs)));
}

/** Where each prop sits in a frame (for LayoutCanvas). */
export function slotsOf(h: PppvHeader): Map<string, PropSlot> {
	const m = new Map<string, PropSlot>();
	let off = 0;
	for (const p of h.props) {
		m.set(p.id, { off, n: p.n, idx: p.idx ? Uint32Array.from(p.idx) : undefined });
		off += p.n * 3;
	}
	return m;
}

/**
 * Random access to preview frames with a small LRU of inflated blocks. `fetchBlock(n)`
 * returns block `n`'s gzip bytes (an HTTP request, or a slice of a local file).
 */
export class PppvReader {
	readonly header: PppvHeader;
	readonly slots: Map<string, PropSlot>;
	#fetch: (n: number) => Promise<Uint8Array>;
	#cache = new Map<number, Uint8Array>();
	#pending = new Map<number, Promise<Uint8Array | null>>();
	#max: number;
	/** Called when a block arrives (redraw) or fails. */
	onblock: ((n: number, error?: Error) => void) | null = null;

	constructor(header: PppvHeader, fetchBlock: (n: number) => Promise<Uint8Array>, lru = 8) {
		this.header = header;
		this.slots = slotsOf(header);
		this.#fetch = fetchBlock;
		this.#max = Math.max(2, lru);
	}

	get durationMs() {
		return this.header.frameCount * this.header.frameMs;
	}

	blockOf(frame: number) {
		return Math.floor(frame / Math.max(1, this.header.blockFrames));
	}

	/** Load block `n` (and keep it cached); resolves null if it fails. */
	load(n: number): Promise<Uint8Array | null> {
		const hit = this.#cache.get(n);
		if (hit) {
			this.#cache.delete(n);
			this.#cache.set(n, hit);
			return Promise.resolve(hit);
		}
		if (n < 0 || n >= this.header.blocks.length) return Promise.resolve(null);
		let p = this.#pending.get(n);
		if (!p) {
			p = this.#fetch(n)
				.then(inflate)
				.then((raw) => {
					this.#cache.set(n, raw);
					while (this.#cache.size > this.#max) {
						const oldest = this.#cache.keys().next().value as number;
						this.#cache.delete(oldest);
					}
					this.onblock?.(n);
					return raw;
				})
				.catch((e: Error) => {
					this.onblock?.(n, e);
					return null;
				})
				.finally(() => this.#pending.delete(n));
			this.#pending.set(n, p);
		}
		return p;
	}

	/**
	 * The frame at `ms` if its block is loaded (else null, and the block is requested).
	 * The next block is prefetched as playback nears the end of this one.
	 */
	frameAt(ms: number): Uint8Array | null {
		const h = this.header;
		const f = frameIndex(h, ms);
		const b = this.blockOf(f);
		const raw = this.#cache.get(b);
		const within = f - b * h.blockFrames;
		if (within > h.blockFrames / 2) void this.load(b + 1);
		if (!raw) {
			void this.load(b);
			return null;
		}
		const start = within * h.frameBytes;
		if (start + h.frameBytes > raw.length) return null;
		return raw.subarray(start, start + h.frameBytes);
	}

	/** Make sure the block for `ms` (and the next) is loaded, e.g. after a seek. */
	async prefetch(ms: number): Promise<void> {
		const b = this.blockOf(frameIndex(this.header, ms));
		await Promise.all([this.load(b), this.load(b + 1)]);
	}
}

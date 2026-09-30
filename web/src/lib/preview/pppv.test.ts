// The browser PPPV reader decodes the fixture written by the Rust encoder
// (crates/pixelplus-core/src/preview.rs `web_fixture_round_trip`).
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { PppvReader, frameIndex, inflate, parsePppv, slotsOf } from './pppv';

/** Channel value the Rust fixture was written with. */
const val = (f: number, c: number) => (f * 7 + c * 13) % 251;

function fixture() {
	const buf = readFileSync(fileURLToPath(new URL('./fixtures/sample.pppv', import.meta.url)));
	return parsePppv(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
}

describe('PPPV reader', () => {
	it('parses the header written by the daemon', () => {
		const { header } = fixture();
		expect(header).toMatchObject({ v: 1, seqId: 'fixture', frameMs: 50, frameCount: 75, blockFrames: 64 });
		expect(header.blocks).toHaveLength(2);
		expect(header.props.map((p) => [p.id, p.n])).toEqual([
			['a', 10],
			['b', 300]
		]);
		expect(header.props[0].idx).toBeUndefined();
		expect(header.props[1].idx).toHaveLength(300);
		expect(header.frameBytes).toBe(310 * 3);
		const slots = slotsOf(header);
		expect(slots.get('a')).toMatchObject({ off: 0, n: 10 });
		expect(slots.get('b')?.off).toBe(30);
		expect(slots.get('b')?.idx?.[299]).toBe(349);
	});

	it('inflates blocks and returns the right frame for a time', async () => {
		const { header, bytes } = fixture();
		let fetches = 0;
		const r = new PppvReader(header, async (n) => {
			fetches++;
			const b = header.blocks[n];
			return bytes.slice(b.offset, b.offset + b.len);
		});
		expect(r.durationMs).toBe(3750);
		expect(r.frameAt(0)).toBeNull(); // not loaded yet: requested
		await r.prefetch(0);
		for (const ms of [0, 49, 50, 1234, 3200, 3749, 99999]) {
			const f = frameIndex(header, ms);
			if (r.frameAt(ms) === null) await r.prefetch(ms);
			const frame = r.frameAt(ms)!;
			expect(frame, `frame at ${ms}`).not.toBeNull();
			const src = f * 2; // 25 ms sequence, 50 ms preview
			expect(frame[0]).toBe(val(src, 0));
			const idx = header.props[1].idx!;
			for (const k of [0, 123, 299]) expect(frame[30 + k * 3 + 1]).toBe(val(src, 30 + idx[k] * 3 + 1));
		}
		expect(fetches).toBe(2);
		expect(frameIndex(header, -5)).toBe(0);
		expect(frameIndex(header, 1e9)).toBe(74);
	});

	it('keeps a small LRU of blocks and reports failures', async () => {
		const { header, bytes } = fixture();
		const many = { ...header, blocks: Array.from({ length: 12 }, () => header.blocks[0]) };
		const loads: number[] = [];
		const r = new PppvReader(
			many,
			async (n) => {
				loads.push(n);
				if (n === 11) throw new Error('offline');
				return bytes.slice(header.blocks[0].offset, header.blocks[0].offset + header.blocks[0].len);
			},
			3
		);
		const errors: number[] = [];
		r.onblock = (n, e) => e && errors.push(n);
		for (const n of [0, 1, 2, 3]) await r.load(n);
		await r.load(3); // cached
		await r.load(0); // evicted: fetched again
		expect(loads).toEqual([0, 1, 2, 3, 0]);
		expect(await r.load(11)).toBeNull();
		expect(errors).toEqual([11]);
		expect(await r.load(99)).toBeNull();
	});

	it('rejects files that are not previews', async () => {
		expect(() => parsePppv(new TextEncoder().encode('NOPE\0\0\0\0').buffer)).toThrow();
		await expect(inflate(new Uint8Array([1, 2, 3]))).rejects.toThrow();
	});
});

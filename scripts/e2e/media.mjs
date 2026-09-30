// Test media for the end-to-end suite: an FSEQ v2 (zstd) writer and a WAV writer,
// dependency-free (Node >= 22.15 for zlib.zstdCompressSync).
import zlib from 'node:zlib';

/**
 * FSEQ v2 with zstd-compressed blocks (the layout xLights writes, see
 * crates/pixelplus-core/src/fseq.rs `FseqWriter`).
 * @param {{channelCount:number, frameMs:number, frames:number, frame:(i:number, buf:Buffer)=>void, mediaFilename?:string, framesPerBlock?:number}} o
 */
export function writeFseq(o) {
	const fpb = o.framesPerBlock ?? 32;
	const blocks = [];
	const buf = Buffer.alloc(o.channelCount);
	for (let first = 0; first < o.frames; first += fpb) {
		const n = Math.min(fpb, o.frames - first);
		const raw = Buffer.alloc(n * o.channelCount);
		for (let k = 0; k < n; k++) {
			buf.fill(0);
			o.frame(first + k, buf);
			buf.copy(raw, k * o.channelCount);
		}
		blocks.push({ first, data: zlib.zstdCompressSync(raw) });
	}
	if (blocks.length > 255) throw new Error('too many blocks; raise framesPerBlock');
	const vars = [];
	const addVar = (code, s) => {
		const d = Buffer.concat([Buffer.from(s, 'utf8'), Buffer.from([0])]);
		const h = Buffer.alloc(4);
		h.writeUInt16LE(4 + d.length, 0);
		h.write(code, 2, 'latin1');
		vars.push(Buffer.concat([h, d]));
	};
	if (o.mediaFilename) addVar('mf', o.mediaFilename);
	addVar('sp', 'PixelPlus e2e');
	const reserved = blocks.length;
	const headerSize = 32 + reserved * 8;
	const varLen = vars.reduce((a, v) => a + v.length, 0);
	const dataOffset = Math.ceil((headerSize + varLen) / 4) * 4;
	const h = Buffer.alloc(dataOffset);
	h.write('PSEQ', 0, 'latin1');
	h.writeUInt16LE(dataOffset, 4);
	h[6] = 0; // minor
	h[7] = 2; // major
	h.writeUInt16LE(headerSize, 8);
	h.writeUInt32LE(o.channelCount, 10);
	h.writeUInt32LE(o.frames, 14);
	h[18] = o.frameMs;
	h[19] = 0;
	h[20] = ((reserved >> 4) & 0xf0) | 1; // 1 = zstd
	h[21] = reserved & 0xff;
	h[22] = 0; // sparse ranges
	h.writeBigUInt64LE(BigInt(Date.now()) * 1000n, 24);
	let p = 32;
	for (const b of blocks) {
		h.writeUInt32LE(b.first, p);
		h.writeUInt32LE(b.data.length, p + 4);
		p += 8;
	}
	for (const v of vars) {
		v.copy(h, p);
		p += v.length;
	}
	return Buffer.concat([h, ...blocks.map((b) => b.data)]);
}

/** 16-bit PCM mono WAV with a sine tone. */
export function writeWav({ seconds, hz = 440, rate = 22050, amplitude = 0.2 }) {
	const n = Math.round(seconds * rate);
	const data = Buffer.alloc(n * 2);
	for (let i = 0; i < n; i++) {
		data.writeInt16LE(Math.round(Math.sin((2 * Math.PI * hz * i) / rate) * amplitude * 32767), i * 2);
	}
	const h = Buffer.alloc(44);
	h.write('RIFF', 0, 'latin1');
	h.writeUInt32LE(36 + data.length, 4);
	h.write('WAVEfmt ', 8, 'latin1');
	h.writeUInt32LE(16, 16);
	h.writeUInt16LE(1, 20); // PCM
	h.writeUInt16LE(1, 22); // mono
	h.writeUInt32LE(rate, 24);
	h.writeUInt32LE(rate * 2, 28);
	h.writeUInt16LE(2, 32);
	h.writeUInt16LE(16, 34);
	h.write('data', 36, 'latin1');
	h.writeUInt32LE(data.length, 40);
	return Buffer.concat([h, data]);
}

/**
 * The known test pattern. Every prop is a solid colour that changes each second;
 * its first pixel encodes the frame number (R = frame & 255, G = frame >> 8,
 * B = 0x5A) so a reader can tell exactly which frame an output shows.
 */
export function patternColor(propIndex, frame, frameMs) {
	const sec = Math.floor((frame * frameMs) / 1000);
	const k = (propIndex * 7 + sec * 3) % PALETTE.length;
	return PALETTE[k];
}
export const PALETTE = [
	[255, 0, 0],
	[0, 255, 0],
	[0, 0, 255],
	[255, 255, 0],
	[0, 255, 255],
	[255, 0, 255],
	[255, 128, 0],
	[128, 0, 255],
	[255, 255, 255],
	[0, 128, 64],
	[200, 40, 90]
];
export const MARKER_B = 0x5a;

/** Byte offset of prop pixel `i` in the fseq frame (honours `channelRuns`), or null. */
export function channelOf(prop, i) {
	const cpp = prop.channelsPerPixel ?? 3;
	if (!prop.channelRuns) return i < prop.pixelCount ? prop.channelStart + i * cpp : null;
	for (const r of prop.channelRuns) {
		if (i >= r.propOffset && i < r.propOffset + r.pixelCount && i < prop.pixelCount)
			return r.channelStart + (i - r.propOffset) * cpp;
	}
	return null;
}

/** Fill one frame for `props` (show.props in show order). */
export function fillFrame(props, frame, frameMs, buf) {
	props.forEach((p, i) => {
		const [r, g, b] = patternColor(i, frame, frameMs);
		for (let px = 0; px < p.pixelCount; px++) {
			const o = channelOf(p, px);
			if (o === null || o + 2 >= buf.length) continue;
			if (px === 0) {
				buf[o] = frame & 255;
				buf[o + 1] = (frame >> 8) & 255;
				buf[o + 2] = MARKER_B;
			} else {
				buf[o] = r;
				buf[o + 1] = g;
				buf[o + 2] = b;
			}
		}
	});
}

/** Channels a frame needs for `props`. */
export function channelCount(props) {
	let n = 0;
	for (const p of props)
		for (let i = 0; i < p.pixelCount; i++) {
			const o = channelOf(p, i);
			if (o !== null) n = Math.max(n, o + 3);
		}
	return n;
}

/**
 * What a node's outputs must show for fseq frame `chan` (ARCHITECTURE §4.1):
 * output-major RGB, `ppo[k]` pixels on output k+1.
 */
export function nodeFrame(props, nodeId, chan, ppo) {
	const starts = [];
	let total = 0;
	for (const n of ppo) {
		starts.push(total);
		total += n * 3;
	}
	const out = Buffer.alloc(total);
	for (const p of props) {
		for (const s of p.segments ?? []) {
			if (s.nodeId !== nodeId || s.output < 1 || s.output > ppo.length) continue;
			for (let k = 0; k < s.pixelCount; k++) {
				const outPix = s.startPixel + (s.reverse ? s.pixelCount - 1 - k : k);
				if (outPix >= ppo[s.output - 1]) continue;
				const src = channelOf(p, s.propOffset + k);
				if (src === null || src + 2 >= chan.length) continue;
				chan.copy(out, starts[s.output - 1] + outPix * 3, src, src + 3);
			}
		}
	}
	return out;
}

/** Decode a `.ppseq` slice (ARCHITECTURE §7.3) → {frameCount, frameUs, ppo, sha, frame(i)}. */
export function readPpseq(buf) {
	if (buf.toString('latin1', 0, 4) !== 'PPSQ' || buf.toString('latin1', buf.length - 4) !== 'PPSQ')
		throw new Error('not a .ppseq file');
	let o = 4;
	const version = buf.readUInt16LE(o);
	o += 2;
	const frameCount = buf.readUInt32LE(o);
	const frameUs = buf.readUInt32LE(o + 4);
	const frameBytes = buf.readUInt32LE(o + 8);
	o += 12;
	const n = buf.readUInt16LE(o);
	o += 2;
	const ppo = [];
	for (let i = 0; i < n; i++, o += 4) ppo.push(buf.readUInt32LE(o));
	const sha = buf.subarray(o, o + 32).toString('hex');
	const nBlocks = buf.readUInt32LE(buf.length - 8);
	const idx = buf.length - 8 - nBlocks * 16;
	const blocks = [];
	for (let b = 0; b < nBlocks; b++) {
		const e = idx + b * 16;
		blocks.push({
			off: Number(buf.readBigUInt64LE(e)),
			len: buf.readUInt32LE(e + 8),
			first: buf.readUInt32LE(e + 12)
		});
	}
	const cache = new Map();
	const block = (b) => {
		if (!cache.has(b)) {
			const x = blocks[b];
			cache.set(b, zlib.zstdDecompressSync(buf.subarray(x.off, x.off + x.len)));
		}
		return cache.get(b);
	};
	return {
		version,
		frameCount,
		frameUs,
		frameBytes,
		ppo,
		sha,
		frame(i) {
			let b = blocks.length - 1;
			while (b > 0 && blocks[b].first > i) b--;
			const data = block(b);
			const k = i - blocks[b].first;
			return data.subarray(k * frameBytes, (k + 1) * frameBytes);
		}
	};
}

import { describe, expect, it } from 'vitest';
import { decodeWav, encodeWav, wavBlob } from './wav';

describe('encodeWav', () => {
	it('writes a valid 16-bit PCM mono header', () => {
		const buf = encodeWav(new Float32Array([0, 0.5, -0.5, 1, -1]), 24000);
		const v = new DataView(buf);
		const tag = (o: number) => String.fromCharCode(...new Uint8Array(buf, o, 4));
		expect(tag(0)).toBe('RIFF');
		expect(v.getUint32(4, true)).toBe(buf.byteLength - 8);
		expect(tag(8)).toBe('WAVE');
		expect(tag(12)).toBe('fmt ');
		expect(v.getUint16(20, true)).toBe(1); // PCM
		expect(v.getUint16(22, true)).toBe(1); // mono
		expect(v.getUint32(24, true)).toBe(24000);
		expect(v.getUint32(28, true)).toBe(48000); // byte rate
		expect(v.getUint16(32, true)).toBe(2);
		expect(v.getUint16(34, true)).toBe(16);
		expect(tag(36)).toBe('data');
		expect(v.getUint32(40, true)).toBe(10);
		expect(buf.byteLength).toBe(54);
		expect([0, 1, 2, 3, 4].map((i) => v.getInt16(44 + 2 * i, true))).toEqual([0, 16384, -16384, 32767, -32768]);
	});

	it('clips out-of-range samples and interleaves stereo', () => {
		const buf = encodeWav([new Float32Array([2, 0]), new Float32Array([-2, 0.25])], 44100);
		const v = new DataView(buf);
		expect(v.getUint16(22, true)).toBe(2);
		expect(v.getUint16(32, true)).toBe(4);
		expect([0, 1, 2, 3].map((i) => v.getInt16(44 + 2 * i, true))).toEqual([32767, -32768, 0, 8192]);
	});

	it('round-trips through decodeWav', () => {
		const x = Float32Array.from({ length: 1000 }, (_, i) => Math.sin(i / 10) * 0.8);
		const { sampleRate, channels } = decodeWav(encodeWav(x, 24000));
		expect(sampleRate).toBe(24000);
		expect(channels).toHaveLength(1);
		for (let i = 0; i < x.length; i++) expect(Math.abs(channels[0][i] - x[i])).toBeLessThan(1e-4);
	});

	it('rejects mismatched channels and makes an audio/wav blob', () => {
		expect(() => encodeWav([new Float32Array(2), new Float32Array(3)], 8000)).toThrow();
		const b = wavBlob(new Float32Array(10), 8000);
		expect(b.type).toBe('audio/wav');
		expect(b.size).toBe(64);
	});
});

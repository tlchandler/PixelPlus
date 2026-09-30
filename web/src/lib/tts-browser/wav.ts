// 16-bit PCM WAV encoding (RIFF/WAVE, little endian).

/** Encode one or more equal-length float channels (-1..1) as a 16-bit PCM WAV file. */
export function encodeWav(channels: Float32Array | Float32Array[], sampleRate: number): ArrayBuffer {
	const chans = Array.isArray(channels) ? channels : [channels];
	if (!chans.length) throw new Error('encodeWav: no channels');
	const frames = chans[0].length;
	if (chans.some((c) => c.length !== frames)) throw new Error('encodeWav: channels differ in length');
	const nch = chans.length;
	const dataBytes = frames * nch * 2;
	const buf = new ArrayBuffer(44 + dataBytes);
	const v = new DataView(buf);
	const str = (o: number, s: string) => {
		for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i));
	};
	str(0, 'RIFF');
	v.setUint32(4, 36 + dataBytes, true);
	str(8, 'WAVE');
	str(12, 'fmt ');
	v.setUint32(16, 16, true); // PCM chunk size
	v.setUint16(20, 1, true); // PCM
	v.setUint16(22, nch, true);
	v.setUint32(24, sampleRate, true);
	v.setUint32(28, sampleRate * nch * 2, true); // byte rate
	v.setUint16(32, nch * 2, true); // block align
	v.setUint16(34, 16, true); // bits per sample
	str(36, 'data');
	v.setUint32(40, dataBytes, true);
	let o = 44;
	for (let i = 0; i < frames; i++) {
		for (let c = 0; c < nch; c++) {
			const s = Math.max(-1, Math.min(1, chans[c][i]));
			v.setInt16(o, s < 0 ? Math.round(s * 0x8000) : Math.round(s * 0x7fff), true);
			o += 2;
		}
	}
	return buf;
}

export function wavBlob(channels: Float32Array | Float32Array[], sampleRate: number): Blob {
	return new Blob([encodeWav(channels, sampleRate)], { type: 'audio/wav' });
}

/** Parse a 16-bit PCM WAV (used by tests and for round-trips). */
export function decodeWav(buf: ArrayBuffer): { sampleRate: number; channels: Float32Array[] } {
	const v = new DataView(buf);
	const tag = (o: number) => String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));
	if (tag(0) !== 'RIFF' || tag(8) !== 'WAVE') throw new Error('not a WAV file');
	let o = 12;
	let nch = 0,
		sampleRate = 0,
		bits = 0;
	while (o + 8 <= buf.byteLength) {
		const id = tag(o);
		const size = v.getUint32(o + 4, true);
		if (id === 'fmt ') {
			nch = v.getUint16(o + 10, true);
			sampleRate = v.getUint32(o + 12, true);
			bits = v.getUint16(o + 22, true);
		} else if (id === 'data') {
			if (bits !== 16) throw new Error('only 16-bit PCM is supported');
			const frames = size / (2 * nch);
			const channels = Array.from({ length: nch }, () => new Float32Array(frames));
			for (let i = 0; i < frames; i++)
				for (let c = 0; c < nch; c++) {
					const s = v.getInt16(o + 8 + (i * nch + c) * 2, true);
					channels[c][i] = s < 0 ? s / 0x8000 : s / 0x7fff;
				}
			return { sampleRate, channels };
		}
		o += 8 + size + (size & 1);
	}
	throw new Error('WAV has no data chunk');
}

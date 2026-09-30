// Max-pool downsampling of camera frames to the decoder's resolution (F6/F7).
// Max (not average) pooling keeps a single distant LED at full brightness.

/** RGBA `sw`×`sh` → luma `w`×`h` taking the brightest luma of each block. */
export function maxPoolLuma(
	rgba: Uint8ClampedArray | Uint8Array,
	sw: number,
	sh: number,
	w: number,
	h: number,
	out?: Uint8Array
): Uint8Array {
	const o = out && out.length === w * h ? out : new Uint8Array(w * h);
	o.fill(0);
	for (let y = 0; y < sh; y++) {
		const oy = Math.min(h - 1, Math.floor((y * h) / sh)) * w;
		let j = y * sw * 4;
		for (let x = 0; x < sw; x++, j += 4) {
			const l = (rgba[j] * 77 + rgba[j + 1] * 150 + rgba[j + 2] * 29) >> 8;
			const i = oy + Math.min(w - 1, Math.floor((x * w) / sw));
			if (l > o[i]) o[i] = l;
		}
	}
	return o;
}

/** Decoder size for a camera aspect: `width` wide (320 by default), even height. */
export function poolSize(videoW: number, videoH: number, width = 320): { w: number; h: number } {
	const w = width;
	const h = Math.max(2, Math.round((width * (videoH || 9)) / (videoW || 16) / 2) * 2);
	return { w, h };
}

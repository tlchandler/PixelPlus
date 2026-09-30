// Pure helpers for driving the Kokoro model (kept free of kokoro-js imports for testing).

export const STYLE_DIM = 256;
export const STYLE_ROWS = 510;
export const MAX_PHONEMES = 500; // kokoro context is 512 tokens incl. 2 pad tokens

/** Offset of the style row kokoro-js uses for an input of `nTokens` (including the 2 pad tokens). */
export function styleOffset(nTokens: number): number {
	return STYLE_DIM * Math.min(Math.max(nTokens - 2, 0), STYLE_ROWS - 1);
}

/** Split a phoneme string into model-sized chunks, preferring sentence, then clause, then word breaks. */
export function splitPhonemes(ph: string, max = MAX_PHONEMES): string[] {
	const out: string[] = [];
	let rest = ph.trim();
	while (rest.length > max) {
		const window = rest.slice(0, max);
		let cut = -1;
		for (const re of [/[.!?…]\s/g, /[,;:—]\s/g, /\s/g]) {
			let m: RegExpExecArray | null;
			while ((m = re.exec(window))) cut = m.index + 1;
			if (cut > max * 0.3) break;
		}
		if (cut <= 0) cut = max;
		out.push(rest.slice(0, cut).trim());
		rest = rest.slice(cut).trim();
	}
	if (rest) out.push(rest);
	return out;
}

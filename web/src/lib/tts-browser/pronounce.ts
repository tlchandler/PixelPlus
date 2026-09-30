// Pronunciation fixes, same semantics as tts/pixelplus_tts/pronounce.py (from fpp-voices):
//  * whole-word matching (no letter/digit/underscore either side, Unicode-aware)
//  * entries with a capital letter are case-sensitive ("LED" leaves "she led" alone);
//    all-lowercase entries match any case
//  * longest phrase first ("Feliz Navidad" before "Navidad")
//  * a replacement between slashes (/noʊˈɛl/) is exact IPA spliced into the phonemes

import { BUILTIN_PRONUNCIATIONS } from './builtin-pronunciations';

export interface Pronunciation {
	word: string;
	say: string;
}
export type Rule = [RegExp, string];

export const IPA_MARK = '\u0000';

function escapeRegExp(s: string): string {
	return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Built-ins overridden by the user's entries (same word spelling). */
export function mergePronunciations(
	user: Pronunciation[] = [],
	builtin: readonly (readonly [string, string])[] = BUILTIN_PRONUNCIATIONS
): [string, string][] {
	const table = new Map<string, string>();
	for (const [w, s] of builtin) table.set(w, s);
	for (const { word, say } of user) {
		const w = (word ?? '').trim();
		const s = (say ?? '').trim();
		if (w && s) table.set(w, s);
	}
	return [...table.entries()];
}

export function compileRules(pairs: [string, string][]): Rule[] {
	return [...pairs]
		.sort((a, b) => b[0].length - a[0].length)
		.map(([src, dst]) => {
			const flags = src !== src.toLowerCase() ? 'gu' : 'giu';
			return [new RegExp(`(?<![\\p{L}\\p{N}_])${escapeRegExp(src)}(?![\\p{L}\\p{N}_])`, flags), dst];
		});
}

export function isIpa(say: string): boolean {
	return say.length >= 2 && say.startsWith('/') && say.endsWith('/');
}

/** Plain replacements go into the text; /ipa/ ones become "\0<n>\0" placeholders. */
export function applyPronunciations(text: string, rules: Rule[]): { text: string; ipa: string[] } {
	const ipa: string[] = [];
	for (const [pattern, replacement] of rules) {
		let rep = replacement;
		if (isIpa(replacement)) {
			pattern.lastIndex = 0;
			if (!pattern.test(text)) continue;
			ipa.push(replacement.slice(1, -1));
			rep = `${IPA_MARK}${ipa.length - 1}${IPA_MARK}`;
		}
		pattern.lastIndex = 0;
		text = text.replace(pattern, () => rep);
	}
	return { text, ipa };
}

/** Text -> phonemes with IPA overrides spliced in. */
export async function toPhonemes(
	text: string,
	rules: Rule[],
	phonemize: (s: string) => Promise<string>
): Promise<string> {
	const { text: t, ipa } = applyPronunciations(text, rules);
	const parts = t.split(IPA_MARK);
	const out: string[] = [];
	for (let i = 0; i < parts.length; i++) {
		const p = parts[i];
		if (!p.trim()) continue;
		out.push(i % 2 ? ipa[Number(p)] : await phonemize(p));
	}
	return out.join(' ');
}

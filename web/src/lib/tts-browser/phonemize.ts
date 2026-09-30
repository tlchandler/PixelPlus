// Text -> Kokoro phonemes, ported from kokoro-js 1.2.1 (src/phonemize.js, Apache-2.0) so
// pronunciation IPA can be spliced between phonemized text runs exactly as kokoro-js would
// phonemize them. eSpeak-NG comes from the `phonemizer` package (loaded lazily).

type PhonemizeFn = (text: string, language?: string) => Promise<string[]>;
let espeak: Promise<PhonemizeFn> | null = null;

function loadEspeak(): Promise<PhonemizeFn> {
	espeak ??= import('phonemizer').then((m) => m.phonemize as PhonemizeFn);
	return espeak;
}

function splitNum(match: string): string {
	if (match.includes('.')) return match;
	if (match.includes(':')) {
		const [h, m] = match.split(':').map(Number);
		if (m === 0) return `${h} o'clock`;
		if (m < 10) return `${h} oh ${m}`;
		return `${h} ${m}`;
	}
	const year = parseInt(match.slice(0, 4), 10);
	if (year < 1100 || year % 1000 < 10) return match;
	const left = match.slice(0, 2);
	const right = parseInt(match.slice(2, 4), 10);
	const suffix = match.endsWith('s') ? 's' : '';
	if (year % 1000 >= 100 && year % 1000 <= 999) {
		if (right === 0) return `${left} hundred${suffix}`;
		if (right < 10) return `${left} oh ${right}${suffix}`;
	}
	return `${left} ${right}${suffix}`;
}

function flipMoney(match: string): string {
	const bill = match[0] === '$' ? 'dollar' : 'pound';
	if (isNaN(Number(match.slice(1)))) return `${match.slice(1)} ${bill}s`;
	if (!match.includes('.')) {
		const suffix = match.slice(1) === '1' ? '' : 's';
		return `${match.slice(1)} ${bill}${suffix}`;
	}
	const [b, c] = match.slice(1).split('.');
	const d = parseInt(c.padEnd(2, '0'), 10);
	const coins = match[0] === '$' ? (d === 1 ? 'cent' : 'cents') : d === 1 ? 'penny' : 'pence';
	return `${b} ${bill}${b === '1' ? '' : 's'} and ${d} ${coins}`;
}

function pointNum(match: string): string {
	const [a, b] = match.split('.');
	return `${a} point ${b.split('').join(' ')}`;
}

export function normalizeText(text: string): string {
	return text
		.replace(/[‘’]/g, "'")
		.replace(/«/g, '“')
		.replace(/»/g, '”')
		.replace(/[“”]/g, '"')
		.replace(/\(/g, '«')
		.replace(/\)/g, '»')
		.replace(/、/g, ', ')
		.replace(/。/g, '. ')
		.replace(/！/g, '! ')
		.replace(/，/g, ', ')
		.replace(/：/g, ': ')
		.replace(/；/g, '; ')
		.replace(/？/g, '? ')
		.replace(/[^\S \n]/g, ' ')
		.replace(/  +/, ' ')
		.replace(/(?<=\n) +(?=\n)/g, '')
		.replace(/\bD[Rr]\.(?= [A-Z])/g, 'Doctor')
		.replace(/\b(?:Mr\.|MR\.(?= [A-Z]))/g, 'Mister')
		.replace(/\b(?:Ms\.|MS\.(?= [A-Z]))/g, 'Miss')
		.replace(/\b(?:Mrs\.|MRS\.(?= [A-Z]))/g, 'Mrs')
		.replace(/\betc\.(?! [A-Z])/gi, 'etc')
		.replace(/\b(y)eah?\b/gi, "$1e'a")
		.replace(/\d*\.\d+|\b\d{4}s?\b|(?<!:)\b(?:[1-9]|1[0-2]):[0-5]\d\b(?!:)/g, splitNum)
		.replace(/(?<=\d),(?=\d)/g, '')
		.replace(/[$£]\d+(?:\.\d+)?(?: hundred| thousand| (?:[bm]|tr)illion)*\b|[$£]\d+\.\d\d?\b/gi, flipMoney)
		.replace(/\d*\.\d+/g, pointNum)
		.replace(/(?<=\d)-(?=\d)/g, ' to ')
		.replace(/(?<=\d)S/g, ' S')
		.replace(/(?<=[BCDFGHJ-NP-TV-Z])'?s\b/g, "'S")
		.replace(/(?<=X')S\b/g, 's')
		.replace(/(?:[A-Za-z]\.){2,} [a-z]/g, (m) => m.replace(/\./g, '-'))
		.replace(/(?<=[A-Z])\.(?=[A-Z])/gi, '-')
		.trim();
}

const PUNCTUATION = ';:,.!?¡¿—…"«»“”(){}[]';
const PUNCTUATION_PATTERN = new RegExp(
	`(\\s*[${PUNCTUATION.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}]+\\s*)+`,
	'g'
);

export function splitPunctuation(text: string): { match: boolean; text: string }[] {
	const out: { match: boolean; text: string }[] = [];
	let prev = 0;
	for (const m of text.matchAll(PUNCTUATION_PATTERN)) {
		const idx = m.index ?? 0;
		if (prev < idx) out.push({ match: false, text: text.slice(prev, idx) });
		if (m[0].length > 0) out.push({ match: true, text: m[0] });
		prev = idx + m[0].length;
	}
	if (prev < text.length) out.push({ match: false, text: text.slice(prev) });
	return out;
}

/** Kokoro-specific fixes applied to eSpeak output (from kokoro-js). */
export function postProcess(ph: string, language: 'a' | 'b'): string {
	let out = ph
		.replace(/kəkˈoːɹoʊ/g, 'kˈoʊkəɹoʊ')
		.replace(/kəkˈɔːɹəʊ/g, 'kˈəʊkəɹəʊ')
		.replace(/ʲ/g, 'j')
		.replace(/r/g, 'ɹ')
		.replace(/x/g, 'k')
		.replace(/ɬ/g, 'l')
		.replace(/(?<=[a-zɹː])(?=hˈʌndɹɪd)/g, ' ')
		.replace(/ z(?=[;:,.!?¡¿—…"«»“” ]|$)/g, 'z');
	if (language === 'a') out = out.replace(/(?<=nˈaɪn)ti(?!ː)/g, 'di');
	return out.trim();
}

/** 'a' = American English (en-us), 'b' = British English. */
export async function phonemize(text: string, language: 'a' | 'b' = 'a', norm = true): Promise<string> {
	if (norm) text = normalizeText(text);
	const espeakPhonemize = await loadEspeak();
	const lang = language === 'a' ? 'en-us' : 'en';
	const parts = await Promise.all(
		splitPunctuation(text).map(async ({ match, text: t }) =>
			match ? t : (await espeakPhonemize(t, lang)).join(' ')
		)
	);
	return postProcess(parts.join(''), language);
}

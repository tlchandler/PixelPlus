import { describe, expect, it } from 'vitest';
import { BUILTIN_PRONUNCIATIONS } from './builtin-pronunciations';
import { applyPronunciations, compileRules, mergePronunciations, toPhonemes } from './pronounce';

// Same cases as tts/tests/test_pronounce.py.
const apply = (pairs: [string, string][], text: string) => applyPronunciations(text, compileRules(pairs));

describe('pronunciations', () => {
	it('matches whole words only', () => {
		expect(apply([['TSO', 'T S O']], "TSO rocks, TSOs don't").text).toBe("T S O rocks, TSOs don't");
		expect(apply([['Sia', 'See-a']], 'Asia and Sia').text).toBe('Asia and See-a');
	});

	it('is case-sensitive for entries with capitals', () => {
		expect(apply([['LED', 'L E D']], 'LED lights; she led the way').text).toBe('L E D lights; she led the way');
	});

	it('matches lowercase entries in any case', () => {
		expect(apply([['xmas', 'Christmas']], 'XMAS and Xmas and xmas').text).toBe(
			'Christmas and Christmas and Christmas'
		);
	});

	it('prefers the longest phrase', () => {
		const r = apply(
			[
				['Navidad', '/nˌɑvidˈɑd/'],
				['Feliz Navidad', '/fəlˈiz nˌɑvidˈɑd/']
			],
			'Feliz Navidad!'
		);
		expect(r.ipa).toEqual(['fəlˈiz nˌɑvidˈɑd']);
		expect(r.text).toBe('\u00000\u0000!');
	});

	it('handles punctuation and unicode word edges', () => {
		const r = apply(
			[
				['Noël', '/noʊˈɛl/'],
				['St. Nick', 'Saint Nick']
			],
			'Joyeux Noël, St. Nick.'
		);
		expect(r.text).toBe('Joyeux \u00000\u0000, Saint Nick.');
		expect(r.ipa).toEqual(['noʊˈɛl']);
		expect(
			apply(
				[
					['w/', 'with'],
					['&', 'and']
				],
				'cocoa w/ marshmallows & cream'
			).text
		).toBe('cocoa with marshmallows and cream');
		// no match inside a longer unicode word
		expect(apply([['Noël', 'X']], 'Noëlle').text).toBe('Noëlle');
	});

	it('skips IPA rules that do not match', () => {
		expect(apply([['Noel', '/noʊˈɛl/']], 'no match here')).toEqual({ text: 'no match here', ipa: [] });
	});

	it('splices IPA between phonemized runs', async () => {
		const rules = compileRules([
			['Noel', '/noʊˈɛl/'],
			['TSO', 'T S O']
		]);
		const got = await toPhonemes('The first Noel by TSO', rules, async (s) => `<${s.trim()}>`);
		expect(got).toBe('<The first> noʊˈɛl <by T S O>');
	});

	it('merges user entries over built-ins', () => {
		const builtin = new Map(BUILTIN_PRONUNCIATIONS.map(([w, s]) => [w, s]));
		expect(builtin.get('Noel')).toBe('/noʊˈɛl/');
		expect(builtin.get('#1')).toBe('number one');
		expect(BUILTIN_PRONUNCIATIONS.length).toBeGreaterThan(90);
		const merged = new Map(
			mergePronunciations([{ word: 'Noel', say: 'No well' }], [['Noel', '/noʊˈɛl/']] as [string, string][])
		);
		expect(merged.get('Noel')).toBe('No well');
	});
});

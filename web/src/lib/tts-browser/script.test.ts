import { describe, expect, it } from 'vitest';
import { formatScript, parseScript, ScriptError } from './script';
import { resolveVoice } from './voices';

// Same fixtures as tts/tests/test_script.py (shared semantics).
const SHOW_INTRO = `# Two-DJ banter.  Format:   voice: line
#
nick: Good evening, and welcome to the show!
holly: You're tuned in to 88.7 FM.
[pause 1.0]
nick!: Now sit back, relax, and enjoy the show!
holly!!: Merry Christmas, everybody!
`;
const resolve = (n: string) => resolveVoice(n).id;

describe('parseScript', () => {
	it('parses the fpp-voices format', () => {
		const lines = parseScript(SHOW_INTRO, resolve);
		expect(lines.map((l) => l.voice)).toEqual(['nick', 'holly', 'nick', 'holly']);
		expect(lines[0]).toEqual({ voice: 'nick', text: 'Good evening, and welcome to the show!', pauseMs: 0 });
		expect(lines[1].pauseMs).toBe(1000);
		expect(lines[2].energy).toBe(1);
		expect(lines[3].energy).toBe(1.5);
	});

	it('resolves aliases and base voices', () => {
		const lines = parseScript('male: hi\nF: hello\nAF_HEART: hey\nHolly: yo', resolve);
		expect(lines.map((l) => l.voice)).toEqual(['nick', 'holly', 'af_heart', 'holly']);
	});

	it('handles pause units and a leading pause', () => {
		const lines = parseScript('[pause 0.5]\nnick: a\n[pause 250ms]\n[pause 1.5s]\n[Pause .5]', resolve);
		expect(lines[0]).toEqual({ voice: '', text: '', pauseMs: 500 });
		expect(lines[1].pauseMs).toBe(250 + 1500 + 500);
	});

	it('keeps colons and asterisks in the text', () => {
		const [ln] = parseScript('holly!: Doors at 5:30, *enjoy the show!*', resolve);
		expect(ln.text).toBe('Doors at 5:30, *enjoy the show!*');
		expect(parseScript('nick!!!: go', resolve)[0].energy).toBe(1.5);
	});

	it('reports errors with line numbers', () => {
		expect(() => parseScript('#1 hit', resolve)).toThrow(ScriptError);
		try {
			parseScript('nick: ok\n\nthis line has no voice', resolve);
		} catch (e) {
			expect((e as ScriptError).line).toBe(3);
		}
		expect(() => parseScript('nick: ok\nrudolph: hi', resolve)).toThrow(/line 2: Unknown voice/);
		expect(() => parseScript('nick:   ', resolve)).toThrow(ScriptError);
	});

	it('round-trips through formatScript', () => {
		const lines = parseScript(SHOW_INTRO, resolve);
		expect(parseScript(formatScript(lines), resolve)).toEqual(lines);
	});
});

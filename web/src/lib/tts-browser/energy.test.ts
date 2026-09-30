import { describe, expect, it } from 'vitest';
import { curveAt, energyCurve, findEmphasis, planLine, punchLift, softCeiling } from './energy';
import { resolveVoice } from './voices';

// Same cases as tts/tests/test_energy.py.
const NICK = resolveVoice('nick');
const HOLLY = resolveVoice('holly');

describe('findEmphasis', () => {
	it.each([
		['Sit back, relax, and enjoy the show!', ['Sit back, relax,', 'and enjoy the show!', '']],
		['Sit back, *relax*, and enjoy the show.', ['Sit back, ', 'relax', ', and enjoy the show.']],
		['Welcome! Merry Christmas, everybody!', ['Welcome! Merry Christmas,', 'everybody!', '']],
		['Enjoy the show tonight everybody', ['Enjoy the', 'show tonight everybody', '']],
		['Go team', ['', 'Go team', '']],
		['First sentence. Second one here now.', ['First sentence.', 'Second one here now.', '']]
	])('%s', (text, expected) => {
		expect(findEmphasis(text)).toEqual(expected);
	});
});

describe('planLine', () => {
	it('keeps normal lines flat and hype speed at the base level', () => {
		let p = planLine(HOLLY, 0.4);
		expect(p.base).toBe(0.4);
		expect(p.hype).toBe(false);
		expect(p.speed).toBeCloseTo(1.05 * (1 + 0.1 * 0.4));
		p = planLine(HOLLY, 1.5);
		expect(p.base).toBe(0.4);
		expect(p.hype).toBe(true);
		expect(p.speed).toBeCloseTo(1.05 * 1.04);
		expect(planLine(NICK, 0)).toEqual({ base: 0, speed: 1.05, hype: false });
		expect(planLine(NICK, 1, 1.2).speed).toBeCloseTo(1.2);
	});
});

describe('energyCurve', () => {
	it('builds into the punchline', () => {
		const c = energyCurve(['aaaaaaaaa', 'bbbbbbbbb', ''], 3.8, 0.4, 1.0);
		const t0 = (3.8 * 9) / 19;
		expect(c.map((p) => p[1])).toEqual([0.4, 0.4, 1.0, 1.0]);
		expect(c[1][0]).toBeCloseTo(t0 - 0.25);
		expect(c[2][0]).toBeCloseTo(t0 + 0.2);
		expect(c[3][0]).toBe(3.8);
		expect(curveAt(c, 0)).toBe(0.4);
		expect(curveAt(c, 3.8)).toBe(1.0);
		expect(curveAt(c, (c[1][0] + c[2][0]) / 2)).toBeCloseTo(0.7);
	});

	it('eases back for a tail and drops duplicate times', () => {
		const c = energyCurve(['aaaa', 'bbbb', 'cccc'], 2.0, 0.4, 1.5);
		expect(c.map((p) => p[1])).toEqual([0.4, 0.4, 1.5, 1.5, 0.4]);
		expect(c[4][0]).toBeCloseTo((2.0 * 9) / 14 + 0.25);
		const d = energyCurve(['', 'bbbbbbbb', ''], 1.0, 0.4, 1.0);
		const times = d.map((p) => p[0]);
		expect(times).toEqual([...new Set(times)].sort((a, b) => a - b));
	});
});

describe('lift & ceiling', () => {
	it('computes the punchline lift like fpp-voices', () => {
		expect(punchLift(50, 46, NICK, 1)).toBeCloseTo(7);
		expect(punchLift(50, 40, NICK, 1.5)).toBe(9);
		expect(punchLift(40, 50, NICK, 1)).toBe(0);
		expect(punchLift(null, 50, NICK, 1)).toBe(0);
	});
	it('soft-limits above the ceiling', () => {
		expect(softCeiling(200, 300)).toBe(200);
		expect(softCeiling(300 * 16, 300)).toBeCloseTo(600);
	});
});

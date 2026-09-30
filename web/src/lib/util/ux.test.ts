import { describe, expect, it } from 'vitest';
import { dayLabel, parseLogLine, parseLogs } from './logs';
import { cToF, fmtTemp, localeTempUnit, wifiQuality } from './units';

describe('log parsing', () => {
	it('reads the daemon ring format', () => {
		const l = parseLogLine('2026-09-30T07:45:12.123Z  WARN   Garage went offline');
		expect(l.level).toBe('warn');
		expect(l.message).toBe('Garage went offline');
		expect(l.time?.toISOString()).toBe('2026-09-30T07:45:12.123Z');
	});
	it('reads journalctl short-iso lines with a tracing target', () => {
		const l = parseLogLine(
			'2026-09-30T02:45:12-0500 pixelplus-main pixelplusd[812]: ERROR pixelplus_daemon::player::engine: audio device busy'
		);
		expect(l.level).toBe('error');
		expect(l.message).toBe('audio device busy');
		expect(l.time?.toISOString()).toBe('2026-09-30T07:45:12.000Z');
	});
	it('sorts newest first and keeps untimed lines last', () => {
		const out = parseLogs(
			[
				'2026-09-30T05:00:00Z  INFO   b',
				'no time here',
				'2026-09-30T07:00:00Z  INFO   c',
				'2026-09-30T06:00:00Z  INFO   a'
			].join('\n')
		);
		expect(out.map((l) => l.message)).toEqual(['c', 'a', 'b', 'no time here']);
	});
	it('labels days', () => {
		const now = new Date(2026, 8, 30, 20);
		expect(dayLabel(new Date(2026, 8, 30, 1), now)).toBe('Today');
		expect(dayLabel(new Date(2026, 8, 29, 23), now)).toBe('Yesterday');
	});
});

describe('units', () => {
	it('picks °F for US visitors and °C elsewhere', () => {
		expect(localeTempUnit('en-US', 'America/Chicago')).toBe('f');
		expect(localeTempUnit('en-GB', 'Europe/London')).toBe('c');
		expect(localeTempUnit('en-CA', 'America/Toronto')).toBe('c');
		// No region in the language: fall back to the time zone.
		expect(localeTempUnit('en', 'America/Denver')).toBe('f');
		expect(localeTempUnit('en', 'America/Toronto')).toBe('c');
	});
	it('formats temperatures', () => {
		expect(cToF(100)).toBe(212);
		expect(fmtTemp(22, 'f')).toBe('72 °F');
		expect(fmtTemp(22, 'c')).toBe('22 °C');
		expect(fmtTemp(null, 'c')).toBe('—');
	});
	it('turns dBm into bars', () => {
		expect(wifiQuality(-48).bars).toBe(4);
		expect(wifiQuality(-71).label).toBe('Fair');
		expect(wifiQuality(-90).bars).toBe(0);
	});
});

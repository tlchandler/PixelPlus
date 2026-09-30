import { describe, expect, it } from 'vitest';
import { sunTime } from './sun';
import { outputLabel, wiringChain, jackOf, repackChain } from './boards';
import { expandSchedule, restingLine } from './schedule';
import { inDateRange, resolveTimeSpec, describeTimeSpec } from './time';
import { fmtDuration } from './format';
import { buildDemoShow } from '$lib/mock/demo';
import { renderEffect, DEFAULT_EFFECT_SCHEMA, defaultParams } from '$lib/effects/render';
import { EFFECT_KINDS } from '$lib/api/types';
import type { Schedule } from '$lib/api/types';

describe('sun', () => {
	it('computes Chicago winter sunset around 4:20 pm CST', () => {
		const d = sunTime(2026, 12, 21, 41.8781, -87.6298, 'sunset')!;
		const localMin = (d.getUTCHours() * 60 + d.getUTCMinutes() - 6 * 60 + 1440) % 1440;
		expect(localMin).toBeGreaterThan(16 * 60 + 10);
		expect(localMin).toBeLessThan(16 * 60 + 35);
	});
});

describe('boards', () => {
	it('labels outputs like the daemon', () => {
		expect(outputLabel('difftxlarge', 1)).toBe('J1-1');
		expect(outputLabel('difftxlarge', 60)).toBe('J15-4');
		expect(outputLabel('difftx', 3)).toBe('Port 3');
		expect(outputLabel('diffsmart', 2)).toBe('Port 2');
		expect(jackOf('difftxlarge', 9)).toBe(3);
	});
	it('describes the wiring chain in plain words', () => {
		const show = buildDemoShow();
		const arch5 = show.props.find((p) => p.name === 'Arch 5')!;
		expect(
			wiringChain(show, arch5.segments[0])
				.map((s) => s.label)
				.join(' › ')
		).toBe('Main Controller › J1 › Front Yard receiver › Port 2 › pixels 51–100');
	});
	it("doesn't repeat the jack for an output without a receiver", () => {
		const show = buildDemoShow();
		const leader = show.nodes.find((n) => n.board === 'difftxlarge')!;
		show.receivers = show.receivers.filter((r) => !(r.nodeId === leader.id && r.jack === 3));
		const seg = {
			nodeId: leader.id,
			output: 10,
			startPixel: 0,
			pixelCount: 5,
			propOffset: 0,
			reverse: false,
			nullPixels: 0
		};
		expect(
			wiringChain(show, seg)
				.map((s) => s.label)
				.join(' › ')
		).toBe(`${leader.name} › J3 · Port 2 › pixels 1–5`);
	});
	it('repacks a chain in order, keeping null pixels', () => {
		const m = repackChain([
			{ propId: 'b', segIndex: 0, pixelCount: 50, nullPixels: 0 },
			{ propId: 'a', segIndex: 0, pixelCount: 30, nullPixels: 2 }
		]);
		expect(m.get('b:0')).toBe(0);
		expect(m.get('a:0')).toBe(52);
	});
});

describe('schedule', () => {
	it('handles date ranges that wrap the year', () => {
		expect(inDateRange(12, 30, { start: '11-25', end: '01-06' })).toBe(true);
		expect(inDateRange(1, 3, { start: '11-25', end: '01-06' })).toBe(true);
		expect(inDateRange(6, 1, { start: '11-25', end: '01-06' })).toBe(false);
	});
	it('lets special nights override regular ones', () => {
		const show = buildDemoShow();
		const occ = expandSchedule(show.schedule, new Date('2026-12-24T12:00:00Z'), 1);
		const winner = occ.find((o) => !o.overridden)!;
		expect(winner.name).toBe('Christmas Eve');
		expect(occ.some((o) => o.overridden)).toBe(true);
	});
	it('the resting line never names a start time that has passed', () => {
		const schedule = {
			enabled: true,
			location: { lat: 40.7, lon: -74, timezone: 'America/New_York' },
			entries: [
				{
					id: 'e',
					name: 'Tonight',
					enabled: true,
					playlistId: 'p',
					days: ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'],
					start: { kind: 'clock', time: '18:45' },
					end: { kind: 'clock', time: '20:15' },
					priority: 0,
					endBehavior: 'stopNow'
				}
			]
		} as unknown as Schedule;
		// 7 pm in New York: the window is on but nothing plays (stopped by hand).
		const during = new Date('2026-09-30T23:00:00Z');
		expect(restingLine(schedule, during)).toMatch(/^Stopped during tonight's show \(on until 8:15/);
		expect(restingLine(schedule, new Date('2026-09-30T20:00:00Z'))).toMatch(
			/^Starts automatically .* at 6:45/
		);
		// Schedule off: nothing starts by itself.
		expect(restingLine({ ...schedule, enabled: false }, during)).toMatch(/^Nothing is scheduled/);
	});
	it('resolves clock times in the show time zone', () => {
		const loc = { lat: 41.88, lon: -87.63, timezone: 'America/Chicago' };
		const d = resolveTimeSpec({ kind: 'clock', time: '22:00' }, 2026, 12, 1, loc)!;
		expect(d.toISOString()).toBe('2026-12-02T04:00:00.000Z');
		expect(describeTimeSpec({ kind: 'sunset', offsetMin: 15 })).toBe('15 min after sunset');
	});
});

describe('effects', () => {
	it('every effect renders non-black pixels with its default params', () => {
		for (const k of EFFECT_KINDS) {
			const out = new Uint8Array(60 * 3);
			let lit = false;
			for (const t of [0.13, 0.5, 1.7, 2.9]) {
				renderEffect(k, defaultParams(DEFAULT_EFFECT_SCHEMA[k]), t, 60, out);
				if (out.some((v) => v > 0)) lit = true;
			}
			expect(lit, k).toBe(true);
		}
	});
});

describe('format', () => {
	it('formats durations', () => {
		expect(fmtDuration(185000)).toBe('3:05');
		expect(fmtDuration(3723000)).toBe('1:02:03');
		expect(fmtDuration(125000, { long: true })).toBe('2 min 5 s');
	});
});

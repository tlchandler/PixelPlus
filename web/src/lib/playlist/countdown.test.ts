import { describe, expect, it } from 'vitest';
import type { Playlist, Prop, Show } from '$lib/api/types';
import { clipStartMs, countdownText, introLeadMs, newCountdownItem, pickMatrix } from './countdown';

describe('countdown helpers', () => {
	it('formats like the daemon', () => {
		expect(countdownText('{s}', 9)).toBe('9');
		expect(countdownText('SHOW IN {s}', 10)).toBe('SHOW IN 10');
		expect(countdownText('{mm}:{ss}', 75)).toBe('01:15');
		expect(countdownText('{s}', -3)).toBe('0');
	});
	it('picks the largest matrix', () => {
		const p = (id: string, n: number, matrix: boolean, kind = 'matrix') =>
			({
				id,
				name: id,
				kind,
				pixelCount: n,
				matrix: matrix ? { width: 4, height: 4, pixelMap: [] } : undefined
			}) as unknown as Prop;
		expect(pickMatrix([p('a', 16, true), p('b', 64, true), p('c', 999, false)])?.id).toBe('b');
		expect(pickMatrix([p('x', 10, false)])).toBeUndefined();
		expect(pickMatrix([p('tree', 500, true, 'tree'), p('m', 16, true)])?.id).toBe('m');
	});
	it('aligns the DJ clip end to zero', () => {
		expect(clipStartMs(10_000, 3_000)).toBe(7_000);
		expect(clipStartMs(10_000, 3_000, 500)).toBe(7_500);
		expect(clipStartMs(5_000, 8_000)).toBe(-3_000);
	});
	it('new items have the defaults', () => {
		const c = newCountdownItem();
		expect(c.type).toBe('countdown');
		expect(c.durationMs).toBe(10_000);
		expect(c.id).toHaveLength(10);
	});
	it('knows the intro length for exact starts', () => {
		const show = {
			sequences: [{ id: 's1', durationMs: 60_000 }],
			media: [{ id: 'm1', durationMs: 4_000 }],
			djClips: [{ id: 'd1', mediaId: 'm1' }]
		} as unknown as Show;
		const pl = {
			intro: [
				{ id: 'a', type: 'pause', durationMs: 2_000 },
				{ id: 'b', type: 'dj', djClipId: 'd1' },
				{ id: 'c', type: 'command', command: 'games.stop' },
				newCountdownItem(10_000)
			]
		} as unknown as Playlist;
		expect(introLeadMs(show, pl)).toBe(16_000);
		expect(introLeadMs(show, { intro: [] } as unknown as Playlist)).toBe(0);
	});
});

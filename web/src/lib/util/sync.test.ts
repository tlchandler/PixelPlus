import { describe, expect, it } from 'vitest';
import type { SyncQuality } from '$lib/api/types';
import { fmtMs, syncGrade, PROTOCOL_VERSION } from './sync';

const q = (over: Partial<SyncQuality> = {}): SyncQuality => ({
	offsetErrorMs: 0.3,
	jitterMs: 0.05,
	driftPpm: 12,
	rttMs: 0.6,
	rttP50Ms: 1.1,
	rttP95Ms: 3,
	lossPct: 0,
	samples: 40,
	kernelTimestamps: true,
	...over
});
const node = (sync: SyncQuality | null, extra = {}) => ({
	online: true,
	sync,
	wifiPowerSave: false,
	protocol: PROTOCOL_VERSION,
	...extra
});

describe('sync quality badge', () => {
	it('grades by the clock error bound', () => {
		expect(syncGrade(node(q()))).toMatchObject({
			level: 'excellent',
			label: 'In sync ±0.3 ms',
			tone: 'green'
		});
		expect(syncGrade(node(q({ offsetErrorMs: 3.2 }))).level).toBe('good');
		const fair = syncGrade(node(q({ offsetErrorMs: 8 })));
		expect(fair).toMatchObject({ level: 'fair', tone: 'accent' });
		expect(fair.tips.join(' ')).toMatch(/5 GHz/);
		expect(syncGrade(node(q({ offsetErrorMs: 30 })), 25).level).toBe('poor');
	});

	it('flags power save, loss and version mismatches', () => {
		const ps = syncGrade(node(q(), { wifiPowerSave: true }));
		expect(ps.level).toBe('poor');
		expect(ps.tips[0]).toMatch(/power saving/);
		const lossy = syncGrade(node(q({ lossPct: 25 })));
		expect(lossy.level).toBe('poor');
		expect(lossy.tips[0]).toMatch(/25 %/);
		expect(syncGrade(node(q(), { protocol: 1 }))).toMatchObject({ level: 'poor', label: 'Update needed' });
	});

	it('explains long strings and waits for data', () => {
		const slow = syncGrade(node(q({ refreshHz: 20 })));
		expect(slow.level).toBe('excellent');
		expect(slow.tips[0]).toMatch(/split long strings/);
		expect(syncGrade(node(null)).level).toBe('unknown');
		expect(syncGrade(node(q(), { online: false })).level).toBe('unknown');
	});

	it('formats milliseconds', () => {
		expect(fmtMs(0.26)).toBe('±0.3 ms');
		expect(fmtMs(12.4)).toBe('±12 ms');
	});
});

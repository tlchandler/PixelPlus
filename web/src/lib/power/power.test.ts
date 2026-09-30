import { describe, expect, it } from 'vitest';
import type { Show } from '$lib/api/types';
import { groupKind, groupLabel, limitingSummary, load, propMaxAmps } from './power';

const show = {
	nodes: [{ id: 'n1', name: 'Garage' }],
	receivers: [{ id: 'r1', name: 'Porch' }],
	powerSupplies: [{ id: 's1', name: 'Garage PSU' }]
} as unknown as Show;

describe('power helpers', () => {
	it('labels limiter groups', () => {
		expect(groupLabel(show, 'port:r1:2')).toBe('Porch · port 2');
		expect(groupLabel(show, 'bus:r1')).toBe('Porch · main fuse');
		expect(groupLabel(show, 'supply:s1')).toBe('Garage PSU');
		expect(groupLabel(show, 'global')).toContain('Whole display');
		expect(groupLabel(null, 'supply:x')).toBe('Power supply');
		expect(groupKind('port:r1:1')).toBe('port');
		expect(groupKind('weird')).toBe('other');
	});
	it('finds the group limiting most', () => {
		const s = limitingSummary(show, [
			{
				nodeId: 'n1',
				groups: [
					{ id: 'supply:s1', amps: 9, budget: 8, scale: 0.8 },
					{ id: 'global', amps: 1, budget: 9, scale: 1 }
				]
			}
		]);
		expect(s).toEqual({ node: 'Garage', group: 'Garage PSU', scale: 0.8 });
		expect(limitingSummary(show, [{ nodeId: 'n1', groups: [] }])).toBeNull();
	});
	it('computes loads and prop current', () => {
		expect(load(3, 6)).toBe(0.5);
		expect(load(null, 6)).toBe(0);
		expect(load(1, 0)).toBe(0);
		expect(propMaxAmps(100, undefined)).toBe(6);
		expect(propMaxAmps(50, 40)).toBe(2);
	});
});

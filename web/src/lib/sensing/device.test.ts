import { describe, expect, it } from 'vitest';
import { modelFromUA } from './device';

describe('phone model', () => {
	it('reads common user agents', () => {
		expect(modelFromUA('Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/128 Mobile')).toBe(
			'Pixel 8'
		);
		expect(modelFromUA('Mozilla/5.0 (Linux; Android 13; SM-S911B Build/TP1A.220624.014) AppleWebKit')).toBe(
			'SM-S911B'
		);
		// Reduced UA (Chrome 110+): no model.
		expect(modelFromUA('Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36')).toBe('Android phone');
		expect(modelFromUA('Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)')).toBe('iPhone');
		expect(modelFromUA('Mozilla/5.0 (X11; Linux x86_64)')).toBe('This device');
	});
});

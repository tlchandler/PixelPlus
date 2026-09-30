import { describe, expect, it } from 'vitest';
import { maxPoolLuma, poolSize } from './pool';

describe('max pooling', () => {
	it('keeps a single bright pixel', () => {
		const sw = 12,
			sh = 6;
		const rgba = new Uint8Array(sw * sh * 4);
		const at = (x: number, y: number) => (y * sw + x) * 4;
		rgba.set([255, 255, 255, 255], at(7, 4));
		const o = maxPoolLuma(rgba, sw, sh, 4, 2);
		expect([...o]).toEqual([0, 0, 0, 0, 0, 0, 255, 0]);
	});
	it('sizes to the aspect', () => {
		expect(poolSize(1280, 720)).toEqual({ w: 320, h: 180 });
		expect(poolSize(640, 480)).toEqual({ w: 320, h: 240 });
	});
});

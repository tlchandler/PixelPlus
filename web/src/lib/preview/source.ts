// Frame sources for LayoutCanvas (F3): the live WebSocket preview of the real lights,
// or a local sequence preview played on this phone only.
import { onPreview } from '$lib/preview';

/** Where a prop's pixels are in a frame. */
export interface PropSlot {
	/** Byte offset of the prop's first RGB triplet. */
	off: number;
	/** Number of RGB triplets. */
	n: number;
	/** Prop pixel index of each triplet (a subsample); absent = pixels 0..n in order. */
	idx?: Uint32Array;
}

export type FrameDrawer = (rgb: Uint8Array, slots: Map<string, PropSlot>) => void;

export interface FrameSource {
	/** `live` = the lights themselves; `preview` = a local rendering (lights untouched). */
	readonly kind: 'live' | 'preview';
	/** Call `draw` with every new frame. Returns the unsubscribe function. */
	subscribe(draw: FrameDrawer): () => void;
}

/** The real display, from the daemon's binary WebSocket preview. */
export const liveSource: FrameSource = {
	kind: 'live',
	subscribe(draw) {
		let cachedFor: Map<string, number> | null = null;
		let slots = new Map<string, PropSlot>();
		return onPreview((rgb, offsets) => {
			if (offsets !== cachedFor) {
				cachedFor = offsets;
				slots = new Map();
				const ids = [...offsets.entries()].sort((a, b) => a[1] - b[1]);
				for (let i = 0; i < ids.length; i++) {
					const [id, off] = ids[i];
					const end = i + 1 < ids.length ? ids[i + 1][1] : rgb.length;
					slots.set(id, { off, n: Math.max(0, Math.floor((end - off) / 3)) });
				}
			}
			draw(rgb, slots);
		});
	}
};

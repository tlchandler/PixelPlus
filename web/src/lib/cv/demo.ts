// Demo camera for the mock backend (F6/F7): turns the show's layout into a
// synthetic yard the simulator can "film", with one deliberate wiring fault
// (a string plugged in backwards) so the review screen has something to show.
import type { Show } from '$lib/api/types';
import { layoutPoint } from './analyze';
import type { SimLed } from './simulate';
import type { MapStart } from './types';

export function demoScene(show: Show, start: MapStart, W: number, H: number): SimLed[] {
	const props = show.props.filter((p) => start.targets.some((t) => t.propIds?.includes(p.id)));
	const pts = new Map<string, [number, number][]>();
	let x0 = Infinity,
		y0 = Infinity,
		x1 = -Infinity,
		y1 = -Infinity;
	props.forEach((p, n) => {
		const l = p.layout ?? { x: (n % 6) * 100, y: Math.floor(n / 6) * 80, w: 80, h: 40, rotation: 0 };
		const list: [number, number][] = [];
		for (let i = 0; i < p.pixelCount; i++) {
			const [x, y] =
				l.points?.length === p.pixelCount
					? layoutPoint(l, i)
					: [l.x + (l.w * i) / Math.max(1, p.pixelCount - 1), l.y + l.h / 2];
			list.push([x, y]);
			x0 = Math.min(x0, x);
			y0 = Math.min(y0, y);
			x1 = Math.max(x1, x);
			y1 = Math.max(y1, y);
		}
		pts.set(p.id, list);
	});
	const s = Math.min((W * 0.86) / Math.max(1, x1 - x0), (H * 0.8) / Math.max(1, y1 - y0));
	const ox = (W - (x1 - x0) * s) / 2,
		oy = (H - (y1 - y0) * s) / 2;
	// The demo fault: one string with a real layout (points) is wired backwards.
	const flipped = props.find(
		(p) =>
			p.pixelCount >= 8 &&
			p.pixelCount <= 150 &&
			p.segments.length === 1 &&
			p.layout?.points?.length === p.pixelCount
	)?.id;
	const leds: SimLed[] = [];
	for (const t of start.targets) {
		for (const p of props) {
			for (const seg of p.segments) {
				if (seg.nodeId !== t.nodeId || seg.output !== t.output) continue;
				const list = pts.get(p.id)!;
				for (let j = 0; j < seg.pixelCount; j++) {
					const rev = seg.reverse !== (p.id === flipped);
					const propPixel = seg.propOffset + (rev ? seg.pixelCount - 1 - j : j);
					const at = list[propPixel];
					if (!at) continue;
					leds.push({
						k: t.k,
						idx: seg.startPixel + j,
						x: ox + (at[0] - x0) * s,
						y: oy + (at[1] - y0) * s,
						amp: 150 + ((propPixel * 37) % 60),
						visible: true
					});
				}
			}
		}
	}
	return leds;
}

/** A still of the synthetic yard (all lights on), as an object URL. */
export async function demoPhoto(leds: SimLed[], W: number, H: number): Promise<string | null> {
	if (typeof document === 'undefined') return null;
	const k = 3;
	const c = document.createElement('canvas');
	c.width = W * k;
	c.height = H * k;
	const ctx = c.getContext('2d');
	if (!ctx) return null;
	const g = ctx.createLinearGradient(0, 0, 0, c.height);
	g.addColorStop(0, '#0b1020');
	g.addColorStop(1, '#030305');
	ctx.fillStyle = g;
	ctx.fillRect(0, 0, c.width, c.height);
	ctx.fillStyle = '#10131c';
	ctx.fillRect(0, c.height * 0.78, c.width, c.height * 0.22);
	for (const l of leds) {
		const r = ctx.createRadialGradient(l.x * k, l.y * k, 0, l.x * k, l.y * k, 7);
		r.addColorStop(0, 'rgba(255,240,210,0.95)');
		r.addColorStop(1, 'rgba(255,200,120,0)');
		ctx.fillStyle = r;
		ctx.fillRect(l.x * k - 7, l.y * k - 7, 14, 14);
	}
	const blob = await new Promise<Blob | null>((res) => c.toBlob(res, 'image/jpeg', 0.85));
	return blob ? URL.createObjectURL(blob) : null;
}

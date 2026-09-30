// Countdown playlist items (F4, WS3): helpers shared by the editor and tests.
// The daemon renders the countdown (`pixelplus_core::effects::countdown`); these
// mirror its text rules so the editor preview says what the matrix will show.
import type { CountdownItem, Playlist, Prop, Show } from '$lib/api/types';
import { newId } from '$lib/util/id';

export const TEXT_PRESETS: { value: string; label: string }[] = [
	{ value: '{s}', label: '10' },
	{ value: '{mm}:{ss}', label: '00:10' }
];

/** Fill in a countdown text template for `seconds` left (as the daemon does). */
export function countdownText(template: string, seconds: number): string {
	const s = Math.max(0, Math.floor(seconds));
	const pad = (n: number) => String(n).padStart(2, '0');
	return template
		.replaceAll('{mm}', pad(Math.floor(s / 60)))
		.replaceAll('{ss}', pad(s % 60))
		.replaceAll('{m}', String(Math.floor(s / 60)))
		.replaceAll('{s}', String(s));
}

/** The matrix the daemon picks when none is chosen: the largest matrix prop. */
export function pickMatrix(props: Prop[]): Prop | undefined {
	return props
		.filter((p) => p.matrix && p.matrix.width > 0 && p.matrix.height > 0)
		.sort(
			(a, b) => Number(b.kind === 'matrix') - Number(a.kind === 'matrix') || b.pixelCount - a.pixelCount
		)[0];
}

/** A new 10-second countdown with the defaults. */
export function newCountdownItem(durationMs = 10_000): CountdownItem {
	return {
		id: newId(),
		type: 'countdown',
		durationMs,
		text: '{s}',
		color: '#ffffff',
		others: 'fill',
		finale: 'flash',
		djOffsetMs: 0,
		tick: false
	};
}

/** Where a countdown's DJ clip starts (ms into the countdown; negative = its
 *  start is cut) so it ends at zero plus the offset. */
export function clipStartMs(durationMs: number, clipMs: number, offsetMs = 0): number {
	return durationMs - clipMs + offsetMs;
}

/** Length of a playlist's intro when known up front (as the daemon's scheduler
 *  computes it for "start exactly on time"): countdowns, pauses, looks (0 = 30 s),
 *  sequences, audio and DJ clips; commands take no time. */
export function introLeadMs(show: Show, pl: Playlist): number {
	const media = (id?: string) => show.media.find((m) => m.id === id)?.durationMs ?? 0;
	return pl.intro.reduce((sum, i) => {
		switch (i.type) {
			case 'countdown':
			case 'pause':
				return sum + i.durationMs;
			case 'effect':
				return sum + (i.durationMs || 30_000);
			case 'sequence':
				return sum + (show.sequences.find((s) => s.id === i.sequenceId)?.durationMs ?? 0);
			case 'media':
				return sum + media(i.mediaId);
			case 'dj':
				return sum + media(show.djClips.find((c) => c.id === i.djClipId)?.mediaId);
			default:
				return sum;
		}
	}, 0);
}

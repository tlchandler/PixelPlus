import { api } from '$lib/api/client';
import { app } from '$lib/stores/app.svelte';
import { confirm, toasts } from '$lib/stores/toasts.svelte';
import { nextShow } from '$lib/util/schedule';
import { fmtDate, fmtTime } from '$lib/util/time';

export async function playerAct(fn: () => Promise<unknown>, what = 'Player command failed') {
	try {
		await fn();
	} catch (e) {
		toasts.error(what, e instanceof Error ? e.message : undefined);
	}
}

export function togglePlay() {
	const st = app.status;
	if (!st || st.state === 'idle') return playerAct(() => api.play({}));
	if (st.state === 'playing') return playerAct(api.pause);
	if (st.state === 'paused') return playerAct(api.resume);
	return playerAct(() => api.stop());
}

/** What "Lights off" means, for tooltips. */
export const LIGHTS_OFF_HELP =
	'Lights off: every light goes dark right away, like a master switch. The show keeps its place — turn the lights back on any time.';

/** Master "Lights off" switch (the daemon calls it blackout). */
export async function setLightsOff(on: boolean) {
	await playerAct(() => api.blackout(on));
	// Turning them off shows the red "Lights are off" banner (no toast needed on top of it).
	if (!on) toasts.success('Lights are back on');
}

/**
 * Stop the show. Stopping a scheduled show ends the night, so it asks first; stopping something
 * started by hand is instant and offers "Play again".
 */
export async function stopShow() {
	const st = app.status;
	if (!st || st.state === 'idle') return;
	if (st.scheduleEntry) {
		const show = app.show;
		const tz = show?.schedule.location.timezone;
		const after = st.scheduleEntry.endsAt ? new Date(st.scheduleEntry.endsAt) : new Date();
		const next = show ? nextShow(show.schedule, new Date(after.getTime() + 60_000)) : undefined;
		const ok = await confirm({
			title: 'Stop tonight’s show?',
			message: next
				? `The lights go dark now. The schedule starts the show again ${fmtDate(new Date(next.start), tz)} at ${fmtTime(new Date(next.start), tz)}.`
				: 'The lights go dark now. You can start the show again from the player at any time.',
			confirmLabel: 'Stop the show',
			danger: true
		});
		if (!ok) return;
		await playerAct(() => api.stop(true));
		return;
	}
	const playlistId = st.playlist?.id;
	await playerAct(() => api.stop(true));
	toasts.push({
		kind: 'info',
		message: 'Show stopped',
		action: playlistId
			? { label: 'Play again', run: () => playerAct(() => api.play({ playlistId })) }
			: undefined
	});
}

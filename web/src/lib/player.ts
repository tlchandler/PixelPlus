import { api } from '$lib/api/client';
import { app } from '$lib/stores/app.svelte';
import { toasts } from '$lib/stores/toasts.svelte';

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

// Tracks background jobs (beat analysis, auto shows, previews) from WS `job` messages,
// with polling as a fallback when the socket is down.
import { SvelteMap } from 'svelte/reactivity';
import type { JobStatus } from '$lib/api/types';
import { app } from '$lib/stores/app.svelte';
import { library } from './api';

/** Latest status by job id (reactive). */
export const jobs = new SvelteMap<string, JobStatus & { subject?: string }>();
let subscribed = false;

function ensure() {
	if (subscribed) return;
	subscribed = true;
	app.onMessage('job', (j) => {
		jobs.set(j.id, j);
		if (j.state === 'done' && (j.kind === 'analysis' || j.kind === 'autoshow')) void app.reloadShow();
	});
}

/** Running or queued job about `subject` (a media or sequence id), if any. */
export function activeJob(
	subject: string,
	kind?: JobStatus['kind']
): (JobStatus & { subject?: string }) | undefined {
	ensure();
	for (const j of jobs.values())
		if (
			j.subject === subject &&
			(!kind || j.kind === kind) &&
			(j.state === 'queued' || j.state === 'running')
		)
			return j;
	return undefined;
}

/** Wait for a job to finish; `onprogress` gets 0..100. Rejects with the job's message on failure. */
export function waitJob(id: string, onprogress?: (pct: number) => void): Promise<JobStatus> {
	ensure();
	return new Promise((resolve, reject) => {
		let done = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const finish = (j: JobStatus) => {
			if (done) return;
			if (j.state !== 'done' && j.state !== 'failed') {
				onprogress?.(j.pct);
				return;
			}
			done = true;
			unsub();
			clearTimeout(timer);
			jobs.set(j.id, j);
			if (j.state === 'done') resolve(j);
			else reject(new Error(j.result?.message ?? 'That didn’t work. Please try again.'));
		};
		const unsub = app.onMessage('job', (j) => {
			if (j.id === id) finish(j);
		});
		const poll = async () => {
			if (done) return;
			try {
				finish(await library.job(id));
			} catch {
				/* keep waiting for the socket */
			}
			if (!done) timer = setTimeout(poll, 2000);
		};
		timer = setTimeout(poll, 2000);
	});
}

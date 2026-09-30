// Shared plumbing for the feature-wave mock endpoints (created by WS0; frozen).
import type { MockServer } from '../server';
import { HttpError } from '../http';
import { newId } from '$lib/util/id';
import type { LogLine } from '$lib/api/types';

export { HttpError };

export type Json = any;
export type Handler = (ctx: {
	params: string[];
	body: Json;
	query: URLSearchParams;
	form?: FormData;
}) => Json | Promise<Json>;

export interface FeatureContext {
	server: MockServer;
	/** `pattern` is a regex source matched against the whole path after /api/v1. */
	route(method: string, pattern: string, h: Handler): void;
	/** Send a WebSocket message `{type, data}` to every open socket. */
	broadcast(type: string, data: unknown): void;
	/** The show changed: bump its version (clients reload it). */
	bump(): void;
	log(level: LogLine['level'], message: string): void;
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
export const nowIso = () => new Date().toISOString();

/** GET/POST/PUT/DELETE on a list of `{id}` entities. */
export function crud<E extends { id: string }>(
	ctx: FeatureContext,
	base: string,
	list: () => E[],
	onChange: () => void = () => ctx.bump()
) {
	const find = (id: string) => {
		const e = list().find((x) => x.id === id);
		if (!e) throw new HttpError(404, 'not_found', `Not found: ${id}`);
		return e;
	};
	ctx.route('GET', base, () => list());
	ctx.route('GET', `${base}/([^/]+)`, ({ params }) => find(params[0]));
	ctx.route('POST', base, ({ body }) => {
		const e = { ...body, id: body.id || newId() } as E;
		list().push(e);
		onChange();
		return e;
	});
	ctx.route('PUT', `${base}/([^/]+)`, ({ params, body }) => {
		const e = find(params[0]);
		Object.assign(e, body, { id: e.id });
		onChange();
		return e;
	});
	ctx.route('DELETE', `${base}/([^/]+)`, ({ params }) => {
		const arr = list();
		const i = arr.findIndex((x) => x.id === params[0]);
		if (i < 0) throw new HttpError(404, 'not_found', 'Not found');
		arr.splice(i, 1);
		onChange();
	});
	return { find };
}

/** Pretend background job: `job` WS messages 0 → 100 %, then `done(result)`. */
export function runJob(
	ctx: FeatureContext,
	kind: 'analysis' | 'autoshow' | 'preview',
	done: () => { sequenceId?: string; message?: string } | void,
	ms = 2400
): string {
	const id = newId();
	const steps = 6;
	for (let i = 0; i <= steps; i++)
		setTimeout(
			() => {
				const last = i === steps;
				const result = last ? (done() ?? undefined) : undefined;
				ctx.broadcast('job', {
					id,
					kind,
					pct: Math.round((i / steps) * 100),
					state: last ? 'done' : 'running',
					result
				});
			},
			(ms / steps) * i
		);
	return id;
}

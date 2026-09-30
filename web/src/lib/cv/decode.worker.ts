/// <reference lib="webworker" />
// Decodes a camera mapping recording off the main thread (F6/F7, WS4).
import { decode, type DecodeOptions, type Recording } from './decode';
import type { Plan } from './mapcode';

declare const self: DedicatedWorkerGlobalScope;

self.onmessage = (e: MessageEvent<{ rec: Recording; plan: Plan; opts?: DecodeOptions }>) => {
	try {
		const res = decode(e.data.rec, e.data.plan, e.data.opts);
		self.postMessage({ res }, res.heat ? [res.heat.buffer] : []);
	} catch (err) {
		self.postMessage({ error: (err as Error).message ?? String(err) });
	}
};

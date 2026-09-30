// WS2 (F2): audio analysis, auto light shows and the job list, for demo mode.
import type { AudioAnalysis, AutoshowStyle, JobStatus, Sequence } from '$lib/api/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError } from './context';

const STYLES: AutoshowStyle[] = [
	{
		id: 'classic',
		name: 'Classic Christmas',
		description: 'Warm reds, greens and gold. Gentle in the verses, big on the chorus.'
	},
	{ id: 'candy', name: 'Candy', description: 'Red and white stripes that bounce on the beat.' },
	{ id: 'rock', name: 'Rock', description: 'Bold colors, fast chases and a hit on every strong beat.' },
	{ id: 'calm', name: 'Calm', description: 'Slow color washes that swell with the music. No flashes.' },
	{ id: 'party', name: 'Party', description: 'Rainbow everything, sparkles on the high notes.' },
	{ id: 'voice', name: 'Speak with lights', description: 'For DJ talk: every prop glows with the voice.' }
];

/** Job statuses by id (the daemon's `GET /jobs/:id`). */
const jobs = new Map<string, JobStatus & { subject?: string }>();

/** A pretend background job: `job` messages 0 → 100 %, then `done()` supplies the result. */
export function mockJob(
	ctx: FeatureContext,
	kind: JobStatus['kind'],
	subject: string,
	done: () => { sequenceId?: string; message?: string } | void,
	ms = 2400
): string {
	const id = newId();
	const steps = 6;
	const emit = (s: JobStatus & { subject?: string }) => {
		jobs.set(id, s);
		ctx.broadcast('job', s);
	};
	emit({ id, kind, pct: 0, state: 'queued', subject });
	for (let i = 1; i <= steps; i++)
		setTimeout(
			() => {
				const last = i === steps;
				const result = last ? (done() ?? undefined) : undefined;
				emit({
					id,
					kind,
					pct: Math.round((i / steps) * 100),
					state: last ? 'done' : 'running',
					result,
					subject
				});
			},
			(ms / steps) * i
		);
	return id;
}

function analysisOf(durationMs: number, bpm: number): AudioAnalysis {
	const period = 60000 / bpm;
	const beats = Array.from({ length: Math.floor((durationMs - 240) / period) }, (_, i) =>
		Math.round(240 + i * period)
	);
	const n = Math.floor(durationMs / 100);
	const third = durationMs / 3;
	const level = (i: number) => (i * 100 < third ? 0.35 : i * 100 < 2 * third ? 0.6 : 0.9);
	const wave = (k: number) =>
		Array.from({ length: n }, (_, i) =>
			Math.round(Math.max(0, Math.min(255, 255 * level(i) * (0.8 + 0.2 * Math.sin(i / (3 + k))))))
		);
	return {
		v: 1,
		sr: 22050,
		hopMs: 11.61,
		bpm,
		bpmConfidence: 0.8,
		tempoCurve: Array.from({ length: Math.ceil(durationMs / 20000) }, () => bpm),
		beats,
		downbeats: beats.filter((_, i) => i % 4 === 0),
		onsets: beats.slice(0, 64).map((ms, i) => ({ ms, strength: 0.5 + (i % 3) / 6, band: i % 3 })),
		energy10Hz: { rms: wave(0), low: wave(3), mid: wave(5), high: wave(7) },
		sections: [
			{ startMs: 0, endMs: Math.round(third), level: 'low' },
			{ startMs: Math.round(third), endMs: Math.round(2 * third), level: 'mid' },
			{ startMs: Math.round(2 * third), endMs: durationMs, level: 'high' }
		]
	};
}

export function register(ctx: FeatureContext) {
	const show = () => ctx.server.show;
	ctx.route('GET', '/autoshow/styles', () => STYLES);
	ctx.route('GET', '/jobs', () =>
		[...jobs.values()].filter((j) => j.state === 'queued' || j.state === 'running')
	);
	ctx.route('GET', '/jobs/([^/]+)', ({ params }) => {
		const j = jobs.get(params[0]);
		if (!j) throw new HttpError(404, 'not_found', 'That job isn’t known.');
		return j;
	});
	ctx.route('GET', '/media/([^/]+)/analysis', ({ params }) => {
		const m = show().media.find((x) => x.id === params[0]);
		if (!m) throw new HttpError(404, 'not_found', 'That audio file');
		if (!m.analysis)
			throw new HttpError(
				404,
				'not_ready',
				'The beat analysis isn’t ready yet. It runs in the background after an upload.'
			);
		return analysisOf(m.durationMs, m.analysis.bpm);
	});
	ctx.route('POST', '/media/([^/]+)/analyze', ({ params }) => {
		const m = show().media.find((x) => x.id === params[0]);
		if (!m) throw new HttpError(404, 'not_found', 'That audio file');
		return {
			jobId: mockJob(ctx, 'analysis', m.id, () => {
				m.analysis = {
					version: 1,
					bpm: m.analysis?.bpm ?? 120,
					bpmConfidence: 0.8,
					beatCount: Math.round((m.durationMs / 60000) * 120),
					firstBeatMs: 250,
					energy: 0.6,
					sections: 3
				};
				ctx.bump();
			})
		};
	});
	const check = (body: any) => {
		const m = show().media.find((x) => x.id === body?.mediaId);
		if (!m) throw new HttpError(400, 'bad_request', 'Pick a song first.');
		const style = STYLES.find((s) => s.id === (body.style ?? 'classic'));
		if (!style) throw new HttpError(400, 'bad_request', `"${body.style}" isn't a style PixelPlus knows.`);
		if (!show().props.length)
			throw new HttpError(
				400,
				'bad_request',
				'There are no props to light yet. Add props on the Props page first.'
			);
		return { m, style, seed: typeof body.seed === 'number' ? body.seed : Math.floor(Math.random() * 1e6) };
	};
	ctx.route('POST', '/autoshow', ({ body }) => {
		const { m, style, seed } = check(body);
		return {
			seed,
			jobId: mockJob(ctx, 'autoshow', m.id, () => {
				const seq: Sequence = {
					id: newId(),
					name: body.name || `${m.name} (${style.name} light show)`,
					file: 'sequences/auto.fseq',
					durationMs: m.durationMs,
					frameMs: 25,
					channelCount: show().sequences[0]?.channelCount ?? 0,
					mediaId: m.id,
					hash: newId(),
					tags: ['auto'],
					generated: {
						kind: style.id === 'voice' ? 'voice' : 'autoShow',
						mediaId: m.id,
						style: style.id,
						propIds: body.propIds ?? [],
						seed,
						analysisVersion: 1,
						propsHash: 'demo'
					}
				};
				show().sequences.push(seq);
				ctx.bump();
				return { sequenceId: seq.id };
			})
		};
	});
	ctx.route('POST', '/autoshow/preview', ({ body }) => {
		const { m, seed } = check(body);
		const sequenceId = `tmp-${newId()}`;
		tempShows.set(sequenceId, m.durationMs);
		return { seed, sequenceId, jobId: mockJob(ctx, 'autoshow', m.id, () => ({ sequenceId }), 1500) };
	});
	ctx.route('POST', '/sequences/([^/]+)/regenerate', ({ params, body }) => {
		const s = show().sequences.find((x) => x.id === params[0]);
		if (!s) throw new HttpError(404, 'not_found', 'That sequence');
		if (!s.generated)
			throw new HttpError(400, 'bad_request', 'Only light shows PixelPlus made can be made again.');
		if (typeof body?.seed === 'number') s.generated.seed = body.seed;
		return {
			jobId: mockJob(ctx, 'autoshow', s.id, () => {
				s.hash = newId();
				ctx.bump();
				return { sequenceId: s.id };
			})
		};
	});
}

/** Temporary auto-show previews: id → duration (served by the preview mock). */
export const tempShows = new Map<string, number>();

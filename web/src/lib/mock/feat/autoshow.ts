// WS2 (F2): audio analysis and auto light shows.
import type { AudioAnalysis, AutoshowStyle, Sequence } from '$lib/api/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { HttpError, runJob } from './context';

const STYLES: AutoshowStyle[] = [
	{
		id: 'classic',
		name: 'Classic Christmas',
		description: 'Warm colors, gentle waves, big finish on the chorus.'
	},
	{ id: 'candy', name: 'Candy', description: 'Red and white stripes that bounce on the beat.' },
	{ id: 'rock', name: 'Rock', description: 'Fast chases and flashes on every strong beat.' },
	{ id: 'calm', name: 'Calm', description: 'Slow color washes that swell with the music.' },
	{ id: 'party', name: 'Party', description: 'Rainbow everything, sparkles on the high notes.' }
];

function analysisOf(durationMs: number, bpm: number): AudioAnalysis {
	const period = 60000 / bpm;
	const beats = Array.from({ length: Math.floor(durationMs / period) }, (_, i) =>
		Math.round(240 + i * period)
	);
	const n = Math.floor(durationMs / 100);
	const wave = (k: number) =>
		Array.from({ length: n }, (_, i) => Math.round(128 + 100 * Math.sin(i / (20 + k))));
	return {
		v: 1,
		sr: 22050,
		hopMs: 11.6,
		bpm,
		bpmConfidence: 0.8,
		tempoCurve: [bpm],
		beats,
		downbeats: beats.filter((_, i) => i % 4 === 0),
		onsets: beats.slice(0, 64).map((ms, i) => ({ ms, strength: 0.5 + (i % 3) / 6, band: i % 3 })),
		energy10Hz: { rms: wave(0), low: wave(3), mid: wave(5), high: wave(7) },
		sections: [
			{ startMs: 0, endMs: Math.round(durationMs / 3), level: 'low' },
			{ startMs: Math.round(durationMs / 3), endMs: Math.round((2 * durationMs) / 3), level: 'mid' },
			{ startMs: Math.round((2 * durationMs) / 3), endMs: durationMs, level: 'high' }
		]
	};
}

export function register(ctx: FeatureContext) {
	const show = () => ctx.server.show;
	ctx.route('GET', '/autoshow/styles', () => STYLES);
	ctx.route('GET', '/media/([^/]+)/analysis', ({ params }) => {
		const m = show().media.find((x) => x.id === params[0]);
		if (!m?.analysis) throw new HttpError(404, 'not_found', 'Not analyzed yet');
		return analysisOf(m.durationMs, m.analysis.bpm);
	});
	ctx.route('POST', '/media/([^/]+)/analyze', ({ params }) => {
		const m = show().media.find((x) => x.id === params[0]);
		if (!m) throw new HttpError(404, 'not_found', 'That audio file');
		return {
			jobId: runJob(ctx, 'analysis', () => {
				m.analysis = {
					version: 1,
					bpm: 120,
					bpmConfidence: 0.8,
					beatCount: 300,
					firstBeatMs: 250,
					energy: 0.6,
					sections: 6
				};
				ctx.bump();
			})
		};
	});
	ctx.route('POST', '/autoshow', ({ body }) => {
		const m = show().media.find((x) => x.id === body?.mediaId);
		if (!m) throw new HttpError(400, 'bad_request', 'Pick a song first.');
		const style = STYLES.find((s) => s.id === body.style) ?? STYLES[0];
		return {
			jobId: runJob(ctx, 'autoshow', () => {
				const seq: Sequence = {
					id: newId(),
					name: `${m.name} (${style.name})`,
					file: 'sequences/auto.fseq',
					durationMs: m.durationMs,
					frameMs: 25,
					channelCount: show().sequences[0]?.channelCount ?? 0,
					mediaId: m.id,
					hash: '',
					tags: ['auto'],
					generated: {
						kind: 'autoShow',
						mediaId: m.id,
						style: style.id,
						propIds: body.propIds ?? [],
						seed: body.seed ?? 1,
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
	ctx.route('POST', '/autoshow/preview', () => ({
		jobId: runJob(ctx, 'preview', () => ({ sequenceId: 'tmp-demo' }))
	}));
	ctx.route('POST', '/sequences/([^/]+)/regenerate', ({ params }) => {
		const s = show().sequences.find((x) => x.id === params[0]);
		if (!s?.generated)
			throw new HttpError(400, 'bad_request', 'Only generated sequences can be regenerated.');
		return { jobId: runJob(ctx, 'autoshow', () => ({ sequenceId: s.id })) };
	});
}

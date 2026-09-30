// Client for the audio-intelligence and library endpoints (F2, F3, F18; WS2).
import { request } from '$lib/api/client';
import type {
	AudioAnalysis,
	AutoshowStyle,
	JobStatus,
	Sequence,
	SmartPreview,
	SmartRules
} from '$lib/api/types';
import type { PppvHeader } from '$lib/preview/pppv';

const get = <R>(p: string) => request<R>('GET', p);
const post = <R>(p: string, b?: unknown) => request<R>('POST', p, b ?? {});

export interface AutoshowRequest {
	mediaId: string;
	style: string;
	/** Empty / absent = all props. */
	propIds?: string[];
	seed?: number;
	name?: string;
}

export interface TagInfo {
	name: string;
	color?: string;
	sequences: number;
	media: number;
}

export interface HistoryRow {
	sequenceId: string;
	plays: number;
	lastPlayed?: string;
}

/** Smart playlist preview with planned start times. */
export interface SmartPreviewFull extends SmartPreview {
	startsAt?: string[];
	start?: string;
	seed?: number;
}

/** `GET /sequences/:id/preview`: the header when ready, else the build job. */
export type PreviewAnswer =
	{ ready: true; header: PppvHeader } | { ready: false; jobId: string; pct: number };

export const library = {
	// ---- F2 analysis + auto shows
	styles: () => get<AutoshowStyle[]>('/autoshow/styles'),
	analysis: (mediaId: string) => get<AudioAnalysis>(`/media/${mediaId}/analysis`),
	analyze: (mediaId: string) => post<{ jobId: string }>(`/media/${mediaId}/analyze`),
	createShow: (r: AutoshowRequest) => post<{ jobId: string; seed: number }>('/autoshow', r),
	previewShow: (r: AutoshowRequest) =>
		post<{ jobId: string; seed: number; sequenceId?: string }>('/autoshow/preview', r),
	regenerate: (sequenceId: string, r: Partial<Omit<AutoshowRequest, 'mediaId'>> = {}) =>
		post<{ jobId: string }>(`/sequences/${sequenceId}/regenerate`, r),
	job: (id: string) => get<JobStatus>(`/jobs/${id}`),

	// ---- F3 preview
	preview: async (sequenceId: string): Promise<PreviewAnswer> => {
		const r = await get<PppvHeader | { jobId: string; pct: number }>(`/sequences/${sequenceId}/preview`);
		return 'jobId' in r ? { ready: false, jobId: r.jobId, pct: r.pct ?? 0 } : { ready: true, header: r };
	},
	previewBlock: async (sequenceId: string, n: number): Promise<Uint8Array> => {
		const b = await request<Blob>('GET', `/sequences/${sequenceId}/preview/block/${n}`, undefined, {
			raw: 'blob'
		});
		return new Uint8Array(await b.arrayBuffer());
	},

	// ---- F18 library
	bulkTags: (ids: string[], add: string[] = [], remove: string[] = []) =>
		post<Sequence[]>('/sequences/tags', { ids, add, remove }),
	tags: () => get<TagInfo[]>('/library/tags'),
	editTag: (name: string, patch: { name?: string; color?: string | null }) =>
		request('PUT', `/library/tags/${encodeURIComponent(name)}`, patch),
	deleteTag: (name: string) => request('DELETE', `/library/tags/${encodeURIComponent(name)}`),
	history: (days = 14) => get<HistoryRow[]>(`/library/history?days=${days}`),
	playlistPreview: (id: string, opts: { date?: string; start?: string; seed?: number } = {}) => {
		const q = new URLSearchParams();
		if (opts.date) q.set('date', opts.date);
		if (opts.start) q.set('start', opts.start);
		if (opts.seed != null) q.set('seed', String(opts.seed));
		const s = q.toString();
		return get<SmartPreviewFull>(`/playlists/${id}/preview${s ? `?${s}` : ''}`);
	},
	rulesPreview: (
		rules: SmartRules,
		opts: { playlistId?: string; date?: string; start?: string; seed?: number } = {}
	) => post<SmartPreviewFull>('/library/smart-preview', { rules, ...opts })
};

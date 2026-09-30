// WS2 (F3): browser preview files (header only in demo mode).
import type { PreviewHeader } from '$lib/api/types';
import type { FeatureContext } from './context';
import { HttpError } from './context';

export function register(ctx: FeatureContext) {
	ctx.route('GET', '/sequences/([^/]+)/preview', ({ params }): PreviewHeader => {
		const s = ctx.server.show.sequences.find((x) => x.id === params[0]);
		if (!s) throw new HttpError(404, 'not_found', 'That sequence');
		const frameMs = Math.max(50, s.frameMs);
		const frameCount = Math.ceil(s.durationMs / frameMs);
		return {
			v: 1,
			seqId: s.id,
			frameMs,
			frameCount,
			props: ctx.server.show.props.map((p) => ({ id: p.id, n: Math.min(p.pixelCount, 300) })),
			blockFrames: 64,
			blocks: []
		};
	});
	ctx.route(
		'GET',
		'/sequences/([^/]+)/preview/data',
		() => new Blob([], { type: 'application/octet-stream' })
	);
}

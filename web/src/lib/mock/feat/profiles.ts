// WS6 (F8): season profiles.
import type { ShowProfile } from '$lib/api/types';
import { newId } from '$lib/util/id';
import type { FeatureContext } from './context';
import { crud, HttpError } from './context';

export function register(ctx: FeatureContext) {
	const show = () => ctx.server.show;
	const list = () => (show().profiles ??= []);
	ctx.route('POST', '/profiles/capture', ({ body }) => {
		const p: ShowProfile = {
			id: newId(),
			name: body?.name || 'New season',
			priority: 0,
			schedule: structuredClone(show().schedule)
		};
		list().push(p);
		ctx.bump();
		return p;
	});
	ctx.route('GET', '/profiles/preview-switch/([^/]+)', ({ params }) => {
		const p = list().find((x) => x.id === params[0]);
		if (!p) throw new HttpError(404, 'not_found', 'That season');
		return {
			lines: [`Schedule: ${show().schedule.entries.length} entries → ${p.schedule.entries.length}`]
		};
	});
	ctx.route('POST', '/profiles/([^/]+)/activate', ({ params }) => {
		const p = list().find((x) => x.id === params[0]);
		if (!p) throw new HttpError(404, 'not_found', 'That season');
		show().activeProfileId = p.id;
		show().schedule = structuredClone(p.schedule);
		ctx.bump();
		return show();
	});
	crud(ctx, '/profiles', list);
}

// WS5 (F15): signed cluster updates, and (F10) controller replacement / transfer, in demo mode.
import type { DiscoveredNode, Node, UpdateInfo, UpdateRun } from '$lib/api/types';
import { newId } from '$lib/util/id';
import { HttpError, nowIso, type FeatureContext } from './context';

export function register(ctx: FeatureContext) {
	const s = ctx.server;
	let current = '0.9.0';
	const latest = '0.9.2';
	let run: UpdateRun | null = null;
	const history: NonNullable<UpdateInfo['history']> = [
		{
			at: '2026-09-02T11:04:00Z',
			from: '0.8.4',
			to: '0.9.0',
			ok: true,
			scope: 'cluster',
			nodes: 3,
			message: '0.8.4 → 0.9.0 on 3 controllers.'
		}
	];
	const settings = () =>
		(s.show.settings.updates ??= {
			channel: 'stable',
			auto: 'notify',
			window: { from: '10:00', to: '14:00', days: [] },
			avoidShowHours: 2
		});
	const nodes = () =>
		s.show.nodes.map((n) => ({
			id: n.id,
			name: n.name,
			version: current,
			proto: 2,
			canApply: true,
			online: true,
			arch: 'arm64',
			phase: 'idle'
		}));

	ctx.route('GET', '/system/update', () => {
		const available = current !== latest;
		return {
			current,
			latest,
			available,
			canApply: available,
			channel: settings().channel,
			ota: true,
			notes:
				'• Faster sequence slicing for followers\n• Fault finder now supports reversed segments\n• Fixes a crash when a follower disconnects mid-song',
			nodes: nodes(),
			problems: [],
			run,
			history,
			previous: history.find((h) => h.ok && h.to === current)?.from
		} satisfies UpdateInfo;
	});

	const progress = (target: string, kind: 'update' | 'rollback') => {
		const job: UpdateRun = {
			id: newId(),
			kind,
			scope: 'cluster',
			version: target,
			from: current,
			phase: kind === 'update' ? 'staging' : 'rollingBack',
			nodes: s.show.nodes.map((n) => ({
				id: n.id,
				name: n.name,
				isSelf: n.role === 'leader',
				arch: 'arm64',
				from: current,
				phase: 'pending'
			})),
			startedAt: nowIso()
		};
		run = job;
		const push = () => ctx.broadcast('updateJob', structuredClone(job));
		const steps: (() => void)[] =
			kind === 'update'
				? [
						() => job.nodes.forEach((n) => (n.phase = 'staging')),
						() => job.nodes.forEach((n) => (n.phase = 'staged')),
						() => {
							job.phase = 'committingFollowers';
							job.nodes
								.filter((n) => !n.isSelf)
								.forEach((n) => ((n.phase = 'committing'), (n.committed = true)));
						},
						() => job.nodes.filter((n) => !n.isSelf).forEach((n) => (n.phase = 'healthy')),
						() => {
							job.phase = 'committingLeader';
							job.nodes
								.filter((n) => n.isSelf)
								.forEach((n) => ((n.phase = 'committing'), (n.committed = true)));
						},
						() => {
							job.nodes.forEach((n) => (n.phase = 'healthy'));
							job.phase = 'done';
							job.finishedAt = nowIso();
							job.message = `${job.from} → ${target} on ${job.nodes.length} controllers.`;
							history.unshift({
								at: nowIso(),
								from: job.from,
								to: target,
								ok: true,
								scope: 'cluster',
								nodes: job.nodes.length,
								message: job.message
							});
							current = target;
							s.toast('success', `Updated: ${job.message}`);
						}
					]
				: [
						() => job.nodes.filter((n) => !n.isSelf).forEach((n) => (n.phase = 'rollingBack')),
						() => job.nodes.filter((n) => !n.isSelf).forEach((n) => (n.phase = 'rolledBack')),
						() => job.nodes.filter((n) => n.isSelf).forEach((n) => (n.phase = 'rollingBack')),
						() => {
							job.nodes.forEach((n) => (n.phase = 'rolledBack'));
							job.phase = 'done';
							job.finishedAt = nowIso();
							job.message = `Back to PixelPlus ${target}.`;
							history.unshift({
								at: nowIso(),
								from: job.from,
								to: target,
								ok: true,
								scope: 'cluster',
								nodes: job.nodes.length,
								message: job.message
							});
							current = target;
						}
					];
		steps.forEach((f, i) =>
			setTimeout(
				() => {
					f();
					push();
				},
				900 * (i + 1)
			)
		);
		push();
		return job;
	};

	ctx.route('POST', '/system/update', () => {
		if (current === latest) throw new HttpError(409, 'conflict', 'PixelPlus is up to date.');
		if (run && !['done', 'rolledBack', 'failed'].includes(run.phase))
			throw new HttpError(409, 'conflict', 'An update is already running.');
		const job = progress(latest, 'update');
		return { ok: true, message: `Updating to PixelPlus ${latest}…`, run: job };
	});
	ctx.route('PUT', '/system/update/settings', ({ body }) => {
		Object.assign(settings(), body ?? {});
		ctx.bump();
		return settings();
	});
	ctx.route('POST', '/system/update/rollback', () => {
		const prev = history.find((h) => h.ok && h.to === current)?.from;
		if (!prev) throw new HttpError(409, 'conflict', 'There’s no earlier version to go back to.');
		return { ok: true, run: progress(prev, 'rollback') };
	});

	// ---- F10: controller replacement and transfer --------------------------------------
	ctx.route('POST', '/nodes/([^/]+)/replace', ({ params, body }) => {
		const node = s.show.nodes.find((n) => n.id === params[0]);
		if (!node) throw new HttpError(404, 'not_found', 'The controller to replace was not found');
		if (node.role === 'leader')
			throw new HttpError(400, 'bad_request', 'The show leader is replaced with a transfer file.');
		const cand = s.discovered.find((d: DiscoveredNode) => d.id === body?.candidateId);
		if (!cand) throw new HttpError(404, 'not_found', 'The new controller is not announcing itself any more.');
		const lost = cand.board !== node.board && cand.board !== 'bare-pi' && cand.board !== 'virtual';
		if (lost && !body?.force)
			throw new HttpError(
				409,
				'board_mismatch',
				`The new controller is a ${cand.board}; ${node.name} was a ${node.board}. Some outputs would be unwired. Confirm to replace it anyway.`
			);
		(node as Node).hardwareHistory = [
			...(node.hardwareHistory ?? []),
			{ at: nowIso(), board: node.board, piModel: node.piModel, serial: node.serial, reason: 'replaced' }
		];
		node.piModel = cand.pi ?? node.piModel;
		node.serial = 'PPX-' + newId().slice(0, 8).toUpperCase();
		s.discovered = s.discovered.filter((d: DiscoveredNode) => d.id !== cand.id);
		ctx.bump();
		s.toast('success', `${node.name} now runs on the new controller`);
		return node;
	});
	ctx.route('POST', '/nodes/([^/]+)/release-retired', ({ params }) => {
		s.discovered = s.discovered.filter((d: DiscoveredNode) => !(d.id === params[0] && d.retired));
		return { ok: true };
	});
	ctx.route('POST', '/system/transfer/export', ({ body }) => {
		if (String(body?.passphrase ?? '').length < 10)
			throw new HttpError(400, 'bad_request', 'Use a passphrase of at least 10 characters.');
		return { url: '/api/v1/system/transfer/download/demo', expiresInS: 600 };
	});
	ctx.route('GET', '/system/transfer/download/demo', () => {
		s.toast('info', 'Demo mode: a real controller downloads the encrypted transfer file (.ppxfer) now.');
		return { ok: true };
	});
	// First-run setup, including "Restore a show from a transfer file" (multipart).
	ctx.route('POST', '/system/setup', async ({ body, form }) => {
		if (form) {
			const pass = String(form.get('passphrase') ?? '');
			const file = form.get('transfer');
			if (!file) throw new HttpError(400, 'bad_request', 'Choose a transfer file (.ppxfer).');
			if (pass.length < 10)
				throw new HttpError(400, 'wrong_passphrase', 'That passphrase doesn’t open this transfer file.');
			await new Promise((r) => setTimeout(r, 1500));
			s.system.needsSetup = false;
			s.system.role = 'leader';
			ctx.bump();
			return {
				...s.system,
				notes: [],
				restored: { showName: s.show.name, hostname: 'pixelplus-main', files: 42 }
			};
		}
		s.system.needsSetup = false;
		s.system.role = body.role;
		if (body.showName) s.show.name = body.showName;
		if (body.board) s.system.board = body.board;
		if (body.location) s.show.schedule.location = body.location;
		if (body.password) s.password = body.password;
		if (body.role === 'follower') s.system.leaderName = undefined;
		ctx.bump();
		return s.system;
	});
}

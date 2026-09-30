/**
 * Turning features on and off from anywhere in the UI (the Features page, a page's "turned
 * off" state, the setup wizard): confirmation when something in use is affected, the daemon
 * call, and a toast with Undo. The rules themselves live in `$lib/features`.
 */
import { api } from '$lib/api/client';
import type { FeatureId } from '$lib/api/types';
import { app } from '$lib/stores/app.svelte';
import { confirm, toasts } from '$lib/stores/toasts.svelte';
import {
	FEATURE_IDS,
	disabledOf,
	feature,
	presetDisabled,
	setFeature,
	usage,
	type PresetId
} from '$lib/features';

const names = (ids: FeatureId[]) => {
	const n = ids.map((id) => feature(id).name);
	return n.length <= 1 ? (n[0] ?? '') : `${n.slice(0, -1).join(', ')} and ${n[n.length - 1]}`;
};

/** Connected over HTTPS right now (turning phone trust off drops this page). */
function onHttps() {
	return typeof location !== 'undefined' && location.protocol === 'https:';
}

/** What a confirmation should say before `ids` go off, or `null` when nothing is at stake. */
export function offMessage(ids: FeatureId[]): string | null {
	const use = usage(app.show);
	const lines: string[] = [];
	for (const id of ids) {
		const f = feature(id);
		const u = use[id];
		if (f.offWarning) lines.push(f.offWarning);
		else if (u.inUse && u.whileOff) lines.push(u.whileOff);
	}
	if (ids.includes('phoneTrust') && onHttps())
		lines.push(
			'You’re connected securely right now: this page stops working at this address. Open it with http:// instead.'
		);
	const used = ids.filter((id) => use[id].inUse);
	if (!lines.length && !used.length) return null;
	return [...lines, 'Nothing is deleted: everything you set up comes back when you turn it on again.'].join(
		' '
	);
}

async function save(body: { id: FeatureId; enabled: boolean } | { disabled: string[] }) {
	const r = await api.setFeatures(body);
	await app.reloadShow();
	return r;
}

function undoAction(previous: string[]) {
	return {
		label: 'Undo',
		run: async () => {
			try {
				await save({ disabled: previous });
			} catch (e) {
				toasts.error('Couldn’t undo that', (e as Error).message);
			}
		}
	};
}

/**
 * Turn one feature on or off (what it needs / what needs it follows). Asks first when
 * turning off something in use. Resolves `true` when the change was made.
 */
export async function toggleFeature(id: FeatureId, on: boolean): Promise<boolean> {
	const before = disabledOf(app.show?.settings);
	const plan = setFeature(before, id, on);
	if (!plan.changed.length) return true;
	const f = feature(id);
	const others = plan.changed.filter((x) => x !== id);
	if (!on) {
		const msg = offMessage(plan.changed);
		const alsoOff = others.length
			? `${names(others)} ${others.length === 1 ? 'needs' : 'need'} it and will turn off too. `
			: '';
		if (msg || alsoOff) {
			const ok = await confirm({
				title: `Turn off ${f.name}?`,
				message: alsoOff + (msg ?? 'Nothing is deleted.'),
				confirmLabel: 'Turn off',
				danger: id === 'power' || id === 'remote'
			});
			if (!ok) return false;
		}
	}
	try {
		await save({ id, enabled: on });
	} catch (e) {
		toasts.error(`Couldn’t turn ${on ? 'on' : 'off'} ${f.name}`, (e as Error).message);
		return false;
	}
	const extra = others.length
		? on
			? ` — ${names(others)} came on too (it needs ${others.length === 1 ? 'it' : 'them'})`
			: ` — ${names(others)} turned off too`
		: '';
	toasts.success(`${f.name} is ${on ? 'on' : 'off'}${extra}`, undoAction(before));
	return true;
}

/** Apply a preset (asks first when it turns off something in use). */
export async function applyPreset(id: Exclude<PresetId, 'custom'>): Promise<boolean> {
	const before = disabledOf(app.show?.settings);
	const next = presetDisabled(id);
	const turningOff = FEATURE_IDS.filter((f) => next.includes(f) && !before.includes(f));
	const use = usage(app.show);
	const used = turningOff.filter((f) => use[f].inUse);
	if (used.length) {
		const ok = await confirm({
			title: id === 'essentials' ? 'Switch to Essentials?' : 'Turn everything on?',
			message: `${names(used)} ${used.length === 1 ? 'is' : 'are'} set up on this controller and will be turned off. ${
				used.includes('power') ? 'Your fuses and power supplies will no longer be protected. ' : ''
			}Nothing is deleted: turn ${used.length === 1 ? 'it' : 'them'} back on any time.`,
			confirmLabel: 'Switch',
			danger: used.includes('power')
		});
		if (!ok) return false;
	}
	try {
		await save({ disabled: [...next, ...before.filter((d) => !FEATURE_IDS.includes(d as FeatureId))] });
	} catch (e) {
		toasts.error('Couldn’t change your features', (e as Error).message);
		return false;
	}
	toasts.success(id === 'essentials' ? 'Switched to Essentials' : 'Everything is on', undoAction(before));
	return true;
}

/**
 * Feature toggles — the single source of truth for Settings → Features (ARCHITECTURE §12.17).
 *
 * Every optional part of PixelPlus has a `FeatureId` (the Rust `pixelplus_core::features`
 * catalogue, same ids, same dependencies). A show keeps the ids that are **off** in
 * `settings.features.disabled`; a show without that key has everything on.
 *
 * Use `isEnabled(id)` (reactive: re-evaluates when the show changes) to gate navigation,
 * routes, cards, buttons and playlist item types, and `featureForPath()` to gate pages.
 * The pure helpers (`setFeature`, `normalize`, `presetOf`, `usage`) are shared with the
 * Features page, the setup wizard and the demo backend.
 */
import type { Component } from 'svelte';
import type { FeatureId, PlaylistItem, Show, ShowSettings } from '$lib/api/types';
import {
	AudioWaveform,
	Bell,
	CalendarRange,
	Camera,
	ClipboardList,
	Gamepad2,
	Globe,
	Hand,
	Hash,
	House,
	Map as MapIcon,
	Mic,
	PartyPopper,
	Radar,
	Router,
	ScanSearch,
	ShieldCheck,
	Sparkles,
	Tags,
	Timer,
	ToggleRight,
	Upload,
	WandSparkles,
	Zap
} from '@lucide/svelte';

export type { FeatureId };
export type FeatureGroupId = 'show' | 'setup' | 'operations';

export interface FeatureDef {
	id: FeatureId;
	name: string;
	/** One plain sentence: what it does for you. */
	description: string;
	icon: Component<any>;
	group: FeatureGroupId;
	/** Features this one needs (turning one of them off turns this off too). */
	requires: FeatureId[];
	/** Pages that belong to this feature (they show "turned off" while it's off). */
	routes: string[];
	/** Extra words the Features page search matches. */
	keywords: string;
	/** Shown in the confirmation when it's turned off. */
	offWarning?: string;
}

export const FEATURE_GROUPS: { id: FeatureGroupId; label: string; blurb: string }[] = [
	{ id: 'show', label: 'Show extras', blurb: 'Things your visitors see and hear' },
	{ id: 'setup', label: 'Setup tools', blurb: 'Helpers for building and fixing your display' },
	{ id: 'operations', label: 'Running the show', blurb: 'Automation, protection and connections' }
];

export const FEATURES: FeatureDef[] = [
	// ---- Show extras
	{
		id: 'dj',
		name: 'DJ Studio',
		description: 'Radio-style announcements between songs, with voices, clips and pronunciations.',
		icon: Mic,
		group: 'show',
		requires: [],
		routes: ['/dj'],
		keywords: 'tts voice announcer speech radio clip kokoro'
	},
	{
		id: 'effects',
		name: 'Effects & looks',
		description: 'Design your own looks and try them live. The built-in idle looks keep working.',
		icon: WandSparkles,
		group: 'show',
		requires: [],
		routes: ['/effects'],
		keywords: 'look effect preset chase twinkle rainbow idle'
	},
	{
		id: 'autoShows',
		name: 'Light shows from music',
		description: 'Beat and tempo analysis builds a light show for your props from any song.',
		icon: Sparkles,
		group: 'show',
		requires: [],
		routes: [],
		keywords: 'auto show generate analysis beat tempo bpm music'
	},
	{
		id: 'smartPlaylists',
		name: 'Smart playlists & tags',
		description: 'Tag your songs and let rules pick tonight’s running order without repeats.',
		icon: Tags,
		group: 'show',
		requires: [],
		routes: [],
		keywords: 'tag rules rotation smart library'
	},
	{
		id: 'countdown',
		name: 'Countdown to showtime',
		description: 'A countdown on your matrix, with the first song starting exactly on the minute.',
		icon: Timer,
		group: 'show',
		requires: [],
		routes: [],
		keywords: 'countdown intro start exact matrix'
	},
	{
		id: 'seasons',
		name: 'Seasons',
		description: 'Keep Halloween and Christmas shows side by side and switch between them by date.',
		icon: CalendarRange,
		group: 'show',
		requires: [],
		routes: ['/settings/seasons'],
		keywords: 'profile halloween christmas season switch'
	},
	{
		id: 'requests',
		name: 'Song requests',
		description: 'A public page and printable yard sign where visitors pick the next song.',
		icon: Hand,
		group: 'show',
		requires: [],
		routes: ['/yard-sign'],
		keywords: 'request vote visitors qr yard sign public'
	},
	{
		id: 'games',
		name: 'Games',
		description: 'Visitors play games on your matrix, using their phone as the controller.',
		icon: Gamepad2,
		group: 'show',
		requires: [],
		routes: ['/games'],
		keywords: 'mario nes arcade rom play matrix'
	},
	// ---- Setup tools
	{
		id: 'layout',
		name: 'Layout & preview',
		description: 'A live picture of your whole display, and sequence previews without the lights.',
		icon: MapIcon,
		group: 'setup',
		requires: [],
		routes: ['/layout'],
		keywords: 'layout preview 2d canvas picture'
	},
	{
		id: 'faultFinder',
		name: 'Fault finder',
		description: 'Answer “is it lit?” a few times to find the first bad pixel on a run.',
		icon: ScanSearch,
		group: 'setup',
		requires: [],
		routes: [],
		keywords: 'fault broken pixel test find repair'
	},
	{
		id: 'pixelCount',
		name: 'Pixel count check',
		description: 'Counts the pixels really on a run and fixes the prop to match.',
		icon: Hash,
		group: 'setup',
		requires: [],
		routes: [],
		keywords: 'count pixels measure run length'
	},
	{
		id: 'receiverWizard',
		name: 'Receiver wizard',
		description: 'A step-by-step guide for plugging in and setting up a new receiver.',
		icon: Router,
		group: 'setup',
		requires: [],
		routes: [],
		keywords: 'receiver guided add diffrx wizard'
	},
	{
		id: 'mapYard',
		name: 'Map my yard',
		description: 'Film the house with your phone to place props and find swapped or reversed runs.',
		icon: Camera,
		group: 'setup',
		requires: ['phoneTrust'],
		routes: ['/map'],
		keywords: 'camera map video place props swapped reversed'
	},
	{
		id: 'soundSync',
		name: 'Sync to sound',
		description: 'Your phone listens and watches to measure the audio delay, so lights match the music.',
		icon: AudioWaveform,
		group: 'setup',
		requires: ['phoneTrust'],
		routes: ['/calibrate'],
		keywords: 'calibrate delay audio sync fm latency microphone'
	},
	{
		id: 'phoneTrust',
		name: 'Phone trust (HTTPS)',
		description: 'A secure connection so phones can use their camera and microphone on PixelPlus pages.',
		icon: ShieldCheck,
		group: 'setup',
		requires: [],
		routes: ['/settings/https', '/trust'],
		keywords: 'https certificate secure tls trust ca'
	},
	// ---- Running the show
	{
		id: 'reports',
		name: 'Nightly report',
		description: 'How last night went, waiting for you every morning — in the app, by email or push.',
		icon: ClipboardList,
		group: 'operations',
		requires: [],
		routes: ['/reports', '/settings/reports'],
		keywords: 'report journal history summary morning'
	},
	{
		id: 'alerts',
		name: 'Alerts',
		description: 'An email or push message when something needs you, like a controller going offline.',
		icon: Bell,
		group: 'operations',
		requires: [],
		routes: [],
		keywords: 'email ntfy push notification alert'
	},
	{
		id: 'power',
		name: 'Power limiter',
		description: 'Keeps every fuse and power supply within its rating, plus late-night dimming.',
		icon: Zap,
		group: 'operations',
		requires: [],
		routes: ['/settings/power'],
		keywords: 'power supply fuse amps limit dim brightness',
		offWarning: 'Your fuses and power supplies will no longer be protected, and late-night dimming stops.'
	},
	{
		id: 'triggers',
		name: 'Buttons & triggers',
		description: 'Physical buttons and web links that start a song, a look or stop the show.',
		icon: ToggleRight,
		group: 'operations',
		requires: [],
		routes: ['/settings/triggers'],
		keywords: 'gpio button trigger http link'
	},
	{
		id: 'sensors',
		name: 'Sensor nodes',
		description: 'ESP32 motion, button and beam sensors placed around the yard.',
		icon: Radar,
		group: 'operations',
		requires: ['triggers'],
		routes: ['/settings/sensors'],
		keywords: 'esp32 motion beam sensor node'
	},
	{
		id: 'surprises',
		name: 'Surprises',
		description: 'Effects that burst over the song that’s playing when a sensor or button fires.',
		icon: PartyPopper,
		group: 'operations',
		requires: ['triggers'],
		routes: [],
		keywords: 'surprise overlay burst motion'
	},
	{
		id: 'mqtt',
		name: 'Home Assistant & MQTT',
		description: 'Control the show and see its status from your smart home.',
		icon: House,
		group: 'operations',
		requires: [],
		routes: [],
		keywords: 'home assistant mqtt smart home automation'
	},
	{
		id: 'remote',
		name: 'Remote access',
		description: 'Reach the show from anywhere with Tailscale or Cloudflare, without port forwarding.',
		icon: Globe,
		group: 'operations',
		requires: [],
		routes: ['/settings/remote'],
		keywords: 'tailscale cloudflare tunnel internet remote',
		offWarning:
			'PixelPlus stops answering through your tunnels: the public request page, games and remote sign-in are unreachable from outside your home network. Your tunnel settings are kept.'
	},
	{
		id: 'xlightsUpload',
		name: 'Upload from xLights',
		description: 'Send sequences straight from xLights (FPP Connect) or a drop folder.',
		icon: Upload,
		group: 'operations',
		requires: [],
		routes: ['/settings/xlights'],
		keywords: 'xlights fpp connect upload drop folder'
	}
];

export const FEATURE_IDS: FeatureId[] = FEATURES.map((f) => f.id);
const BY_ID = new Map(FEATURES.map((f) => [f.id, f]));

export function feature(id: FeatureId): FeatureDef {
	return BY_ID.get(id)!;
}

/** Features that need `id`. */
export function dependents(id: FeatureId): FeatureId[] {
	return FEATURES.filter((f) => f.requires.includes(id)).map((f) => f.id);
}

/** Always there, whatever is turned off. */
export const ALWAYS_ON: string[] = [
	'Dashboard',
	'Props',
	'Controllers',
	'Sequences & Audio',
	'Playlists',
	'Schedule',
	'Settings',
	'Backups',
	'Updates'
];

// ------------------------------------------------------------------ presets

export type PresetId = 'essentials' | 'everything' | 'custom';

/** "Essentials": the everyday setup tools a new display needs, plus the power limiter. */
export const ESSENTIALS: FeatureId[] = [
	'effects',
	'layout',
	'faultFinder',
	'pixelCount',
	'receiverWizard',
	'power'
];

export const PRESETS: { id: Exclude<PresetId, 'custom'>; label: string; blurb: string }[] = [
	{ id: 'essentials', label: 'Essentials', blurb: 'The basics for a great show, nothing more' },
	{ id: 'everything', label: 'Everything', blurb: 'Every tool and every page' }
];

/** The `disabled` list of a preset. */
export function presetDisabled(id: Exclude<PresetId, 'custom'>): string[] {
	return id === 'everything' ? [] : normalize(FEATURE_IDS.filter((f) => !ESSENTIALS.includes(f)));
}

/** Which preset a `disabled` list is (unknown ids from a newer PixelPlus are ignored). */
export function presetOf(disabled: readonly string[] | undefined): PresetId {
	const known = (disabled ?? []).filter((d) => BY_ID.has(d as FeatureId));
	const key = (xs: readonly string[]) => [...xs].sort().join(',');
	if (!known.length) return 'everything';
	if (key(known) === key(presetDisabled('essentials'))) return 'essentials';
	return 'custom';
}

// ------------------------------------------------------------------ state helpers

export function disabledOf(settings: Pick<ShowSettings, 'features'> | null | undefined): string[] {
	return settings?.features?.disabled ?? [];
}

/** Whether `id` is on in `settings` (no `features` key = everything on). */
export function enabledIn(
	settings: Pick<ShowSettings, 'features'> | null | undefined,
	id: FeatureId
): boolean {
	return !disabledOf(settings).includes(id);
}

/** Sorted, deduplicated, and every feature whose requirement is off turned off too. */
export function normalize(disabled: readonly string[]): string[] {
	const off = new Set(disabled.map((d) => d.trim()).filter(Boolean));
	for (;;) {
		const extra = FEATURES.filter((f) => !off.has(f.id) && f.requires.some((r) => off.has(r)));
		if (!extra.length) break;
		for (const f of extra) off.add(f.id);
	}
	return [...off].sort();
}

/**
 * Turn one feature on or off, keeping dependencies consistent (on: what it needs comes on;
 * off: what needs it goes off). `changed` lists every feature whose state changed, in
 * catalogue order — the same answer as `PUT /features {id, enabled}`.
 */
export function setFeature(
	disabled: readonly string[],
	id: FeatureId,
	on: boolean
): { disabled: string[]; changed: FeatureId[] } {
	const off = new Set(disabled);
	const todo: FeatureId[] = [id];
	while (todo.length) {
		const f = todo.pop()!;
		if (on) {
			if (off.delete(f) || f === id) todo.push(...feature(f).requires);
		} else if (!off.has(f) || f === id) {
			off.add(f);
			todo.push(...dependents(f));
		}
	}
	const next = normalize([...off]);
	const was = new Set(disabled);
	const now = new Set(next);
	const changed = FEATURE_IDS.filter((f) => was.has(f) !== now.has(f));
	return { disabled: next, changed };
}

// ------------------------------------------------------------------ reactive access

let source: () => Show | null | undefined = () => undefined;

/** Called once by the app store so `isEnabled` follows the live show (reactively). */
export function bindFeatureSource(fn: () => Show | null | undefined) {
	source = fn;
}

/**
 * Whether a feature is on for the current show. Reactive inside components and `$derived`
 * (it reads the app's show). Before the show has loaded everything counts as on.
 */
export function isEnabled(id: FeatureId): boolean {
	return enabledIn(source()?.settings, id);
}

/** The feature a page belongs to (`/settings/seasons` → `seasons`), if any. */
export function featureForPath(pathname: string): FeatureId | undefined {
	const path = pathname.replace(/\/+$/, '') || '/';
	for (const f of FEATURES) for (const r of f.routes) if (path === r || path.startsWith(r + '/')) return f.id;
	return undefined;
}

/** A page link is shown only while its feature (if any) is on. */
export function hrefEnabled(href: string): boolean {
	const f = featureForPath(href.split(/[?#]/)[0]);
	return !f || isEnabled(f);
}

/**
 * The feature an API path (relative to `/api/v1`) belongs to — mirrors the daemon's
 * `api::features::feature_for` (used by the demo backend to answer `feature_disabled`).
 */
export function featureForApi(method: string, path: string): FeatureId | undefined {
	const seg = path
		.replace(/^\/api\/v1/, '')
		.replace(/^\//, '')
		.split('/');
	const [a, b, c] = seg;
	const read = method === 'GET' || method === 'HEAD';
	if (['dj-clips', 'dj-voices', 'tts', 'pronunciations'].includes(a)) return 'dj';
	if (a === 'effects' && b === 'preview-apply') return 'effects';
	if (a === 'effects' && !read && !['builtin', 'catalog', 'schema'].includes(b ?? '')) return 'effects';
	if (a === 'autoshow' || a === 'jobs') return 'autoShows';
	if (a === 'media' && b && (c === 'analyze' || c === 'analysis')) return 'autoShows';
	if (a === 'sequences' && b && c === 'regenerate') return 'autoShows';
	if (a === 'sequences' && b === 'tags' && c === undefined) return 'smartPlaylists';
	if (a === 'library') return 'smartPlaylists';
	if (a === 'playlists' && b && c === 'preview') return 'smartPlaylists';
	if (a === 'sequences' && b && c === 'preview') return 'layout';
	if (a === 'profiles') return 'seasons';
	if (a === 'requests' || (a === 'public' && b === 'requests')) return 'requests';
	if (a === 'games') return 'games';
	if (a === 'faultfinder') return 'faultFinder';
	if (a === 'pixelcount') return 'pixelCount';
	if (a === 'wizard' && b === 'receiver') return 'receiverWizard';
	if (a === 'mapping') return 'mapYard';
	if (a === 'calibration' || (a === 'player' && b === 'calibration')) return 'soundSync';
	if (a === 'tls' || (a === 'public' && ['ca.crt', 'ca.mobileconfig', 'tls'].includes(b ?? '')))
		return 'phoneTrust';
	if (a === 'reports') return 'reports';
	if (a === 'alerts') return 'alerts';
	if (a === 'power' && (b === 'budget' || b === 'live')) return 'power';
	if (a === 'triggers' && b) return 'triggers';
	if (a === 'sensor-nodes' || (a === 'cluster' && b === 'sensor-config')) return 'sensors';
	if (a === 'surprises' || (a === 'player' && b === 'surprise' && c === undefined)) return 'surprises';
	if (a === 'mqtt') return 'mqtt';
	if (a === 'remote') return 'remote';
	if (a === 'xlights') return 'xlightsUpload';
	return undefined;
}

/**
 * The feature a playlist item belongs to (DJ clips, countdowns, looks, game commands) —
 * mirrors `pixelplus_core::features::playlist_item_feature`. The show skips these items
 * while their feature is off.
 */
export function itemFeature(it: Pick<PlaylistItem, 'type'> & { command?: string }): FeatureId | undefined {
	switch (it.type) {
		case 'dj':
			return 'dj';
		case 'countdown':
			return 'countdown';
		case 'effect':
			return 'effects';
		case 'command':
			return it.command?.startsWith('games.') ? 'games' : undefined;
		default:
			return undefined;
	}
}

// ------------------------------------------------------------------ usage

export interface FeatureUsage {
	/** Something of this feature is set up or in use (turning it off deserves a warning). */
	inUse: boolean;
	/** Short facts: "3 clips", "used in 2 playlists". */
	facts: string[];
	/** What happens to it while off ("DJ items in 2 playlists are skipped"). */
	whileOff?: string;
}

const n = (count: number, one: string, many = one + 's') => `${count} ${count === 1 ? one : many}`;

/** Playlists that contain an item matching `pred` (items, intro, outro, smart pins). */
function playlistsWith(show: Show, pred: (type: string, it: any) => boolean): number {
	return show.playlists.filter((p) => {
		const items = [
			...p.items,
			...p.intro,
			...p.outro,
			...(p.smart?.pinnedFirst ?? []),
			...(p.smart?.pinnedLast ?? []),
			...(p.smart?.interleave ?? [])
		];
		return items.some((it) => pred(it.type, it));
	}).length;
}

/** "In use" facts per feature, computed from the show (and, for games, the ROM count). */
export function usage(
	show: Show | null | undefined,
	extra: { gameRoms?: number } = {}
): Record<FeatureId, FeatureUsage> {
	const out = Object.fromEntries(
		FEATURE_IDS.map((id): [FeatureId, FeatureUsage] => [id, { inUse: false, facts: [] }])
	) as Record<FeatureId, FeatureUsage>;
	if (!show) return out;
	const s = show.settings;
	const put = (id: FeatureId, facts: (string | false | 0 | undefined | null)[], whileOff?: string) => {
		const f = facts.filter(Boolean) as string[];
		out[id] = { inUse: f.length > 0, facts: f, whileOff: f.length ? whileOff : undefined };
	};

	const djPl = playlistsWith(show, (t, it) => t === 'dj' || (t === 'countdown' && !!it.djClipId));
	put(
		'dj',
		[
			show.djClips.length && n(show.djClips.length, 'clip'),
			show.djVoices.length && n(show.djVoices.length, 'voice'),
			djPl && `used in ${n(djPl, 'playlist')}`
		],
		djPl ? `DJ clips in ${n(djPl, 'playlist')} are skipped while it’s off.` : undefined
	);

	const lookPl = playlistsWith(show, (t) => t === 'effect');
	put(
		'effects',
		[show.effects.length && n(show.effects.length, 'look'), lookPl && `used in ${n(lookPl, 'playlist')}`],
		lookPl
			? `Looks in ${n(lookPl, 'playlist')} are skipped while it’s off; idle looks keep working.`
			: undefined
	);

	const generated = show.sequences.filter((q) => q.generated).length;
	const analysed = show.media.filter((m) => m.analysis).length;
	put('autoShows', [
		generated && n(generated, 'generated show'),
		analysed && `${n(analysed, 'song')} analysed`
	]);

	const smart = show.playlists.filter((p) => p.smart).length;
	const tags = new Set([
		...show.sequences.flatMap((q) => q.tags ?? []),
		...show.media.flatMap((m) => m.tags ?? [])
	]).size;
	put(
		'smartPlaylists',
		[smart && n(smart, 'smart playlist'), tags && n(tags, 'tag')],
		smart ? `${n(smart, 'smart playlist')} will have no songs while it’s off.` : undefined
	);

	const cdPl = playlistsWith(show, (t) => t === 'countdown');
	put(
		'countdown',
		[cdPl && `used in ${n(cdPl, 'playlist')}`],
		cdPl ? `Countdowns in ${n(cdPl, 'playlist')} are skipped while it’s off.` : undefined
	);

	const profiles = show.profiles?.length ?? 0;
	put(
		'seasons',
		[profiles && n(profiles, 'season')],
		profiles ? 'Automatic season switching pauses and every prop lights while it’s off.' : undefined
	);

	put(
		'requests',
		[s.requests.enabled && 'open to visitors'],
		'The public request page and yard sign stop working while it’s off.'
	);

	put(
		'games',
		[s.games.enabled && 'on for visitors', extra.gameRoms && n(extra.gameRoms, 'ROM')],
		'Any game in progress ends and the phone controller page closes.'
	);

	const camera = show.props.filter((p) => p.layout?.source === 'camera').length;
	put('mapYard', [camera && `${n(camera, 'prop')} placed by camera`]);

	const measured = show.nodes.reduce((a, nd) => a + nd.outputs.filter((o) => o.measuredPixels).length, 0);
	put('pixelCount', [measured && `${n(measured, 'run')} counted`]);

	put('soundSync', [s.audio.lastCalibration && 'sound delay measured']);

	const alertsSet = !!(s.alerts.email?.smtpHost || s.alerts.ntfy?.topic);
	put('alerts', [
		alertsSet &&
			[s.alerts.email?.smtpHost && 'email', s.alerts.ntfy?.topic && 'push'].filter(Boolean).join(' and ') +
				' set up'
	]);

	put('reports', [
		s.reports?.enabled && `every morning at ${s.reports.time === 'afterShow' ? 'show end' : s.reports.time}`
	]);

	const supplies = show.powerSupplies?.length ?? 0;
	put('power', [
		supplies && n(supplies, 'power supply', 'power supplies'),
		(s.power?.dim?.length ?? 0) > 0 && 'late-night dimming'
	]);

	const triggers = s.triggers.length;
	put(
		'triggers',
		[triggers && n(triggers, 'trigger')],
		triggers ? 'Buttons and links stop firing while it’s off.' : undefined
	);

	const nodes = show.sensorNodes?.length ?? 0;
	put(
		'sensors',
		[nodes && n(nodes, 'sensor node')],
		nodes ? 'Sensor nodes are ignored while it’s off.' : undefined
	);

	const surprises = s.triggers.filter((t) => t.action.type === 'surprise').length;
	put('surprises', [surprises && n(surprises, 'surprise trigger')]);

	put(
		'mqtt',
		[s.mqtt.enabled && `connected to ${s.mqtt.host}`],
		'PixelPlus disconnects from your smart home while it’s off.'
	);

	const tunnels = [s.remote?.tailscale && 'Tailscale', s.remote?.cloudflare && 'Cloudflare'].filter(Boolean);
	put('remote', [
		tunnels.length > 0 && `${tunnels.join(' and ')} set up`,
		s.remote?.publicListener && 'public pages on'
	]);

	put('xlightsUpload', [s.xlights?.fppConnect && 'FPP Connect on', s.xlights?.watchFolder && 'drop folder']);

	return out;
}

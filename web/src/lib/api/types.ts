// TypeScript mirror of crates/pixelplus-core/src/model.rs (JSON is camelCase)
// plus the HTTP/WS payloads described in docs/ARCHITECTURE.md §8.

export type Id = string;

// ---------------------------------------------------------------- hardware
export type BoardKind = 'difftx' | 'difftxlarge' | 'diffsmart' | 'bare-pi' | 'virtual';
export type NodeRole = 'leader' | 'follower';
export type ColorOrder = 'RGB' | 'RBG' | 'GRB' | 'GBR' | 'BRG' | 'BGR';
export const COLOR_ORDERS: ColorOrder[] = ['RGB', 'RBG', 'GRB', 'GBR', 'BRG', 'BGR'];

export interface OutputConfig {
	index: number;
	label: string;
	pixelType: 'ws2811';
	colorOrder: ColorOrder;
	brightness: number;
	gamma: number;
	enabled: boolean;
}

export interface Node {
	id: Id;
	name: string;
	hostname: string;
	role: NodeRole;
	board: BoardKind;
	boardRev?: string;
	piModel?: string;
	outputs: OutputConfig[];
	adopted: boolean;
	lastSeen?: string;
	notes?: string;
}

export type ReceiverKind = 'diffrx' | 'diffsmart-rx' | 'generic-4' | 'direct';

export interface Receiver {
	id: Id;
	name: string;
	kind: ReceiverKind;
	nodeId: Id;
	jack: number;
	location?: string;
	fuseAmps?: number;
	notes?: string;
}

// ---------------------------------------------------------------- props
export type PropKind =
	| 'arch'
	| 'candycane'
	| 'tree'
	| 'matrix'
	| 'line'
	| 'circle'
	| 'star'
	| 'spinner'
	| 'window'
	| 'icicles'
	| 'custom'
	| 'other';

export const PROP_KINDS: PropKind[] = [
	'arch',
	'candycane',
	'tree',
	'matrix',
	'line',
	'circle',
	'star',
	'spinner',
	'window',
	'icicles',
	'custom',
	'other'
];

export interface PropSegment {
	nodeId: Id;
	output: number;
	startPixel: number;
	pixelCount: number;
	propOffset: number;
	reverse: boolean;
	nullPixels: number;
}

export interface PropLayout {
	x: number;
	y: number;
	w: number;
	h: number;
	rotation: number;
	points?: [number, number][];
}

export interface MatrixInfo {
	width: number;
	height: number;
	pixelMap: number[];
}

export interface Prop {
	id: Id;
	name: string;
	kind: PropKind;
	pixelCount: number;
	xlightsModel?: string;
	channelStart: number;
	channelsPerPixel: number;
	segments: PropSegment[];
	groupIds: Id[];
	layout?: PropLayout;
	matrix?: MatrixInfo;
	color?: string;
	maxMilliampsPerPixel?: number;
	notes?: string;
}

export interface PropGroup {
	id: Id;
	name: string;
	propIds: Id[];
	color?: string;
}

// ---------------------------------------------------------------- content
export interface Sequence {
	id: Id;
	name: string;
	file: string;
	durationMs: number;
	frameMs: number;
	channelCount: number;
	mediaId?: Id;
	xlightsName?: string;
	thumbnail?: string;
	hash: string;
}

export type MediaKind = 'song' | 'dj' | 'sfx';

export interface Media {
	id: Id;
	name: string;
	kind: MediaKind;
	file: string;
	durationMs: number;
	loudnessLufs?: number;
	gainDb?: number;
}

export interface DjLine {
	voice: string;
	text: string;
	pauseMs: number;
	energy?: number;
}

export interface DjVoice {
	id: Id;
	name: string;
	description: string;
	blend: Record<string, number>;
	speed: number;
	lang: string;
	eq?: string;
	defaultEnergy: number;
	energy: Record<string, number>;
}

export interface DjClip {
	id: Id;
	name: string;
	lines: DjLine[];
	dynamic: boolean;
	mediaId?: Id;
	speed: number;
	musicBedMediaId?: Id;
}

export interface Pronunciation {
	word: string;
	say: string;
}

export const DJ_PLACEHOLDERS = [
	'time',
	'date',
	'day',
	'daysUntilChristmas',
	'nextSong',
	'prevSong',
	'showName',
	'temperature',
	'sunset',
	'requestName'
] as const;

export type EffectKind =
	| 'solid'
	| 'chase'
	| 'twinkle'
	| 'rainbow'
	| 'colorwash'
	| 'candycane'
	| 'fire'
	| 'snow'
	| 'sparkle'
	| 'wave'
	| 'meteor'
	| 'strobe'
	| 'breathe';

export const EFFECT_KINDS: EffectKind[] = [
	'solid',
	'chase',
	'twinkle',
	'rainbow',
	'colorwash',
	'candycane',
	'fire',
	'snow',
	'sparkle',
	'wave',
	'meteor',
	'strobe',
	'breathe'
];

export type ParamValue = number | string | boolean | string[];
export type EffectParams = Record<string, ParamValue>;

export interface Target {
	all?: boolean;
	propIds?: Id[];
	groupIds?: Id[];
}

export interface EffectPreset {
	id: Id;
	name: string;
	effect: EffectKind;
	params: EffectParams;
	target: Target;
}

/** GET /effects/schema → Record<EffectKind, ParamSpec[]> */
export interface ParamSpec {
	key: string;
	label: string;
	kind: 'color' | 'colors' | 'number' | 'bool' | 'select';
	min?: number;
	max?: number;
	step?: number;
	default: ParamValue;
	options?: string[];
	/** Unit shown after numbers ("px/s", "Hz", "%", "s"). */
	unit?: string;
	/** One-sentence explanation. */
	help?: string;
}
export type EffectSchema = Record<string, ParamSpec[]>;

export type PlaylistItem =
	| { id: Id; type: 'sequence'; sequenceId: Id }
	| { id: Id; type: 'dj'; djClipId: Id }
	| { id: Id; type: 'effect'; effectId: Id; durationMs: number }
	| { id: Id; type: 'media'; mediaId: Id }
	| { id: Id; type: 'pause'; durationMs: number }
	| { id: Id; type: 'command'; command: string; args?: unknown };

export type PlaylistItemType = PlaylistItem['type'];

export interface Playlist {
	id: Id;
	name: string;
	items: PlaylistItem[];
	intro: PlaylistItem[];
	outro: PlaylistItem[];
	shuffle: boolean;
	repeat: boolean;
	crossfadeMs: number;
}

// ---------------------------------------------------------------- schedule
export interface Location {
	lat: number;
	lon: number;
	timezone: string;
	label?: string;
}

export type TimeSpec =
	| { kind: 'clock'; time: string }
	| { kind: 'sunset'; offsetMin: number }
	| { kind: 'sunrise'; offsetMin: number };

export type Weekday = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';
export const WEEKDAYS: Weekday[] = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'];

export interface DateRange {
	start: string;
	end: string;
}

export type EndBehavior = 'finishSong' | 'stopNow' | 'fadeOut';

export interface ScheduleEntry {
	id: Id;
	name: string;
	enabled: boolean;
	playlistId: Id;
	days: Weekday[];
	dateRange?: DateRange;
	start: TimeSpec;
	end: TimeSpec;
	priority: number;
	endBehavior: EndBehavior;
}

export interface VolumeCurfew {
	time: TimeSpec;
	volume: number;
}

export interface Schedule {
	enabled: boolean;
	location: Location;
	entries: ScheduleEntry[];
	idleEffectId?: Id;
	offEffectId?: Id;
	volumeCurfew?: VolumeCurfew;
}

export interface ScheduleOccurrence {
	date: string; // YYYY-MM-DD
	start: string; // ISO datetime
	end: string; // ISO datetime
	entryId: Id;
	playlistId: Id;
	name: string;
}

// ---------------------------------------------------------------- settings
export interface AudioSettings {
	device: string;
	volume: number;
	normalize: boolean;
	targetLufs: number;
}
export interface EmailSettings {
	smtpHost: string;
	smtpPort: number;
	username: string;
	password: string;
	from: string;
	to: string;
	tls: boolean;
}
export interface NtfySettings {
	server: string;
	topic: string;
}
export interface AlertRules {
	tempC: number;
	voltageMin: number;
	followerOffline: boolean;
	showFailure: boolean;
}
export interface AlertSettings {
	email?: EmailSettings;
	ntfy?: NtfySettings;
	rules: AlertRules;
}
export interface MqttSettings {
	enabled: boolean;
	host: string;
	port: number;
	username?: string;
	password?: string;
	baseTopic: string;
	homeAssistantDiscovery: boolean;
}
export interface RequestSettings {
	enabled: boolean;
	maxQueue: number;
	playlistId?: Id;
	title: string;
	message: string;
}
export type TtsMode = 'auto' | 'device' | 'browser';
export interface TriggerAction {
	type: 'playPlaylist' | 'playSequence' | 'stop' | 'effect';
	ref?: string;
}
export interface Trigger {
	id: Id;
	name: string;
	kind: 'gpio' | 'http';
	gpio?: number;
	action: TriggerAction;
}
export type GamePlayWindow = 'duringShow' | 'anytime';
export type InviteStyle = 'text' | 'qr' | 'alternate';
export type ScaleMode = 'fit' | 'stretch';
export interface GameSettings {
	enabled: boolean;
	matrixPropId?: Id;
	port: number;
	gameSeconds: number;
	cooldownMinutes: number;
	levels: string;
	playWindow: GamePlayWindow;
	pauseShow: boolean;
	santaHat: boolean;
	arcadeMode: boolean;
	arcadeMinutes: number;
	arcadeIdleSeconds: number;
	publicUrl: string;
	inviteEveryMinutes: number;
	inviteStyle: InviteStyle;
	inviteFlashes: number;
	inviteColor: string;
	scaleMode: ScaleMode;
	outputFps: number;
	brightness: number;
	volume: number;
	crop: [number, number, number, number];
}

export interface ShowSettings {
	audio: AudioSettings;
	alerts: AlertSettings;
	mqtt: MqttSettings;
	requests: RequestSettings;
	tts: { mode: TtsMode };
	oled: { enabled: boolean };
	security: { passwordHash?: string };
	triggers: Trigger[];
	games: GameSettings;
}

export interface Show {
	version: number;
	name: string;
	nodes: Node[];
	receivers: Receiver[];
	props: Prop[];
	propGroups: PropGroup[];
	sequences: Sequence[];
	media: Media[];
	djClips: DjClip[];
	djVoices: DjVoice[];
	pronunciations: Pronunciation[];
	effects: EffectPreset[];
	playlists: Playlist[];
	schedule: Schedule;
	settings: ShowSettings;
}

// ---------------------------------------------------------------- runtime / API
export interface SystemInfo {
	version: string;
	nodeId: Id;
	role: NodeRole | 'unconfigured';
	hostname: string;
	board: BoardKind | null;
	boardRev?: string;
	piModel?: string;
	uptimeS: number;
	cpuPct: number;
	memPct: number;
	diskFreeMb: number;
	tempC?: number;
	ips: string[];
	time: string;
	timezone: string;
	wifi?: { ssid: string; signal: number } | null;
	needsSetup: boolean;
	passwordSet: boolean;
	/** Assumed extension: board detected from EEPROM (null = blank EEPROM). */
	detectedBoard?: BoardKind | null;
	/** Assumed extension: leader this follower was adopted by. */
	leaderName?: string;
	/** Running in a container (Docker / Podman). */
	docker?: boolean;
	/** What this installation may change on the host (signed-in only). */
	platform?: PlatformCaps;
	/** DPI string length vs. the boot configuration (signed-in only). */
	outputGeometry?: OutputGeometry;
}

/** How pixelplusd may do privileged things here (see services/platform.rs). */
export interface PlatformCaps {
	/** Running as root (development). */
	root: boolean;
	/** The root helper (pixelplus-helper@.service) can be used. */
	helper: boolean;
	/** Reboot / power off / restart are possible. */
	power: boolean;
	/** PIXELPLUS_BOARD override, if set. */
	boardOverride?: BoardKind | null;
}

/** Progress of a root helper job (`helper` WebSocket message, GET /system/helpers). */
export interface HelperStatus {
	verb: 'config-txt' | 'update' | 'ssh-on' | 'ssh-off' | 'reapply' | 'wifi-country' | 'hosts' | string;
	state: 'running' | 'ok' | 'failed';
	message: string;
	/** Unix seconds. */
	updatedAt: number;
}

/** GET /system/output-geometry. */
export interface OutputGeometry {
	/** False when a string is longer than the pixel output was set up for at boot. */
	ok: boolean;
	longestString: number;
	/** Pixels per output of the running DPI mode (null: not DPI / unknown). */
	maxPixels?: number | null;
	/** Pixels per output the boot configuration on disk is set up for. */
	configuredPixels?: number | null;
	/** The boot configuration already fits; only a restart is missing. */
	pendingReboot: boolean;
	/** "Apply & reboot" is possible here. */
	canApply: boolean;
	targetPixels?: number | null;
	piMaxPixels?: number | null;
	message?: string | null;
}

export interface SshState {
	/** null: unknown (not a PixelPlus Pi). */
	enabled: boolean | null;
	canChange: boolean;
	job?: HelperStatus | null;
}

export interface SetupRequest {
	role: NodeRole;
	showName?: string;
	board?: BoardKind;
	boardRev?: string;
	location?: Location;
	timezone?: string;
	password?: string;
	/** Assumed extension: also write the EEPROM when it was blank. */
	writeEeprom?: boolean;
}

export interface NetworkConfig {
	hostname: string;
	wifi: { ssid: string; psk?: string; country: string };
	ethernet: { dhcp: boolean; address?: string; gateway?: string; dns?: string };
	/** Read-only: false when this machine's network isn't PixelPlus's to change (Docker, PC). */
	managed?: boolean;
	/** Read-only: setup-hotspot watchdog (image/netwatch); null when it isn't running. */
	netwatch?: NetwatchStatus | null;
}

/** /run/pixelplus/netwatch.json, written by the setup-hotspot watchdog. */
export interface NetwatchStatus {
	state: 'waiting' | 'online' | 'hotspot' | 'connecting' | string;
	/** The setup hotspot's name while it is up (PixelPlus-XXXX). */
	hotspotSsid?: string | null;
	/** The hotspot has a password (default "pixelplus"). */
	hotspotSecured: boolean;
	/** Setup page for phones on the hotspot (http://10.42.0.1/). */
	portalUrl?: string | null;
	lastError?: string | null;
	/** Last network joined from the setup page. */
	lastJoined?: { ssid: string; ips: string[]; at?: number | null } | null;
	/** Unix seconds. */
	updatedAt?: number | null;
}
export interface WifiNetwork {
	ssid: string;
	signal: number;
	secure: boolean;
}

export interface Sensor {
	id: string;
	label: string;
	kind: 'temperature' | 'voltage' | 'current' | 'power';
	value: number;
	unit: string;
	warn?: number;
	crit?: number;
	/** Assumed extension: which node reports it (leader when absent). */
	nodeId?: Id;
}

export interface SensorHistory {
	series: Record<string, [number, number][]>;
}

export interface DiscoveredNode {
	id: Id;
	name: string;
	role: string;
	board: BoardKind;
	boardRev?: string;
	pi?: string;
	ver?: string;
	http?: number;
	ip?: string;
	adoptedBy?: string | null;
}

export type PlayerState = 'idle' | 'playing' | 'paused' | 'testing' | 'effect';
export interface ItemRef {
	type: string;
	id: Id;
	name: string;
}
export interface PlayerStatus {
	state: PlayerState;
	playlist?: { id: Id; name: string; index: number; count: number };
	item?: ItemRef;
	posMs: number;
	durationMs: number;
	volume: number;
	brightness: number;
	nextItem?: ItemRef;
	scheduleEntry?: { id: Id; name: string; endsAt: string };
	nextShow?: { name: string; startsAt: string };
	fps: number;
	/** Assumed extension: true while blackout is engaged. */
	blackout?: boolean;
}

export interface NodeStatus {
	id: Id;
	name: string;
	online: boolean;
	lastSeen: string;
	board: BoardKind;
	syncOffsetMs: number;
	syncState: 'synced' | 'syncing' | 'offline';
	files: { pending: number; total: number };
}

export interface LogLine {
	level: 'debug' | 'info' | 'warn' | 'error';
	message: string;
	time: string;
}

export interface ToastMsg {
	kind: 'info' | 'success' | 'warning' | 'error';
	message: string;
}

export type TestMode = 'solid' | 'chase' | 'rgbCycle' | 'countPixels' | 'walk' | 'effect';
export interface TestRequest {
	mode: TestMode;
	color?: string;
	target: { nodeId?: Id; output?: number; propIds?: Id[]; groupIds?: Id[]; all?: boolean };
	effect?: EffectPreset;
}

export interface FaultStep {
	session: string;
	/** range currently lit (0-based inclusive start, exclusive end) */
	litFrom: number;
	litTo: number;
	step: number;
	totalSteps: number;
	question: string;
	done?: boolean;
	result?: { pixelIndex: number | null; message: string };
}

export interface PowerEstimate {
	perOutput: { nodeId: Id; output: number; peakAmps: number; avgAmps: number }[];
	perReceiverPort: { receiverId: Id; port: number; peakAmps: number; avgAmps: number; fuseAmps?: number }[];
	perProp: { propId: Id; peakAmps: number; avgAmps: number }[];
	warnings: string[];
}

export interface HealthCheck {
	id: string;
	label: string;
	status: 'ok' | 'warn' | 'fail';
	detail: string;
	/** A fix the UI can offer: `applyOutputGeometry` (Apply & reboot) or `reboot`. */
	action?: 'applyOutputGeometry' | 'reboot' | string;
}
export interface HealthReport {
	ok: boolean;
	ranAt?: string;
	checks: HealthCheck[];
}

export interface Snapshot {
	id: string;
	label: string;
	createdAt: string;
	sizeBytes: number;
	showVersion?: number;
	auto?: boolean;
}

export interface TtsStatus {
	mode: 'device' | 'browser';
	available: boolean;
	voices: { id: string; name: string; language: string; gender: 'male' | 'female' }[];
}

export interface PublicRequests {
	title: string;
	message: string;
	enabled: boolean;
	showName?: string;
	songs: { sequenceId: Id; name: string; durationMs: number; artist?: string }[];
	queue: { id: string; sequenceId: Id; name: string; requestedBy?: string }[];
	nowPlaying?: { name: string; posMs: number; durationMs: number } | null;
	maxQueue: number;
}

export interface SongRequest {
	id: string;
	sequenceId: Id;
	name: string;
	requestedBy?: string;
	requestedAt: string;
}

export interface GamesStatus {
	enabled: boolean;
	running: boolean;
	arcade: boolean;
	queueLength: number;
	cooldownS: number;
	player?: string;
	lastError?: string;
	/** Assumed extension: sidecar reachable / installed */
	available?: boolean;
	roms?: { name: string; sizeBytes: number }[];
}

export interface UpdateInfo {
	current: string;
	latest: string;
	available: boolean;
	/** The update can be installed from the web UI here. */
	canApply?: boolean;
	notes?: string;
	channel?: string;
	/** Why it can't be installed here / how to update instead. */
	message?: string;
	/** The latest install run, if any. */
	job?: HelperStatus | null;
}

export interface ImportPreview {
	/** In the preview, `segments[].nodeId` holds the xLights controller name until applied. */
	props: Prop[];
	controllers: {
		name: string;
		suggestedNodeId?: Id;
		ip?: string;
		protocol?: string;
		ports: number;
		propCount?: number;
	}[];
	groups?: PropGroup[];
	warnings: string[];
}

export interface ApiErrorBody {
	error: { code: string; message: string };
}

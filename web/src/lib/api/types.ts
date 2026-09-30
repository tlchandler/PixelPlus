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
	/** Pixels found by a pixel-count check (F7). */
	measuredPixels?: MeasuredCount;
}

/** Result of a pixel-count check (F7). */
export interface MeasuredCount {
	count: number;
	method: 'camera' | 'manual' | 'current';
	/** ISO time. */
	at: string;
	/** Output pixel indices that did not respond. */
	dead?: number[];
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
	/** EEPROM serial of the current hardware (F10). */
	serial?: string;
	/** Earlier hardware of this controller (F10 "Replace with…"). */
	hardwareHistory?: HardwareRecord[];
}

export interface HardwareRecord {
	at: string;
	serial?: string;
	board: BoardKind;
	piModel?: string;
	reason: string;
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
	/** Main fuse / bus rating in A (diffrx rev C: 30), F12. */
	mainFuseAmps?: number;
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
	/** Where the layout came from (absent = unknown). */
	source?: LayoutSource;
}
export type LayoutSource = 'xlights' | 'manual' | 'camera';

export interface MatrixInfo {
	width: number;
	height: number;
	pixelMap: number[];
}

/** Prop pixels `propOffset .. propOffset + pixelCount` read from `channelStart` (0-based byte offset). */
export interface ChannelRun {
	propOffset: number;
	channelStart: number;
	pixelCount: number;
}

export interface Prop {
	id: Id;
	name: string;
	kind: PropKind;
	pixelCount: number;
	xlightsModel?: string;
	channelStart: number;
	channelsPerPixel: number;
	/** Non-contiguous channels (xLights individual start channels); absent = contiguous from channelStart. */
	channelRuns?: ChannelRun[];
	segments: PropSegment[];
	groupIds: Id[];
	layout?: PropLayout;
	matrix?: MatrixInfo;
	color?: string;
	maxMilliampsPerPixel?: number;
	notes?: string;
	/** Prop pixel indices flagged dead / suspect (F6, F7, fault finder). */
	suspectPixels?: number[];
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
	/** Made by PixelPlus: auto light show / speak with lights (F2). */
	generated?: GeneratedInfo;
	/** Library tags (F18), e.g. "kids", "season:halloween". */
	tags?: string[];
}

export type GeneratedKind = 'autoShow' | 'voice';
export interface GeneratedInfo {
	kind: GeneratedKind;
	mediaId: Id;
	style: string;
	/** Empty = all props. */
	propIds: Id[];
	seed: number;
	analysisVersion: number;
	propsHash: string;
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
	/** Beat / tempo / energy summary (F2). */
	analysis?: AudioAnalysisSummary;
	/** Library tags (F18). */
	tags?: string[];
	/** File name / size as uploaded (xLights FPP Connect, F16). */
	originalName?: string;
	originalSize?: number;
}

export interface AudioAnalysisSummary {
	version: number;
	bpm: number;
	bpmConfidence: number;
	beatCount: number;
	firstBeatMs: number;
	/** 0..1 mean normalized energy. */
	energy: number;
	sections: number;
}

/** GET /media/:id/analysis (F2), versioned. */
export interface AudioAnalysis {
	v: number;
	sr: number;
	hopMs: number;
	bpm: number;
	bpmConfidence: number;
	tempoCurve: number[];
	beats: number[];
	downbeats: number[];
	onsets: { ms: number; strength: number; band: number }[];
	energy10Hz: { rms: number[]; low: number[]; mid: number[]; high: number[] };
	sections: { startMs: number; endMs: number; level: 'low' | 'mid' | 'high' }[];
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
	| 'breathe'
	/** Show-start countdown (F4); rendered from its playlist item, not in the catalogue. */
	| 'countdown';

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
	| { id: Id; type: 'command'; command: string; args?: unknown }
	| CountdownItem;

/** Show-start countdown (F4), usually the last intro item. */
export interface CountdownItem {
	id: Id;
	type: 'countdown';
	durationMs: number;
	matrixPropId?: Id;
	/** `{s}`, `{mm}:{ss}` or custom text around them; default "{s}". */
	text?: string;
	color?: string;
	others?: 'fill' | 'pulse' | 'dark';
	finale?: 'flash' | 'none';
	djClipId?: Id;
	/** 0 = the clip ends at zero. */
	djOffsetMs?: number;
	tick?: boolean;
}

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
	/** Smart playlist rules (F18): items are generated at play time. */
	smart?: SmartRules;
}

export type TagMatch = 'any' | 'all';
export type SmartOrder = 'leastRecent' | 'shuffle' | 'rotation' | 'fixed';
export interface SmartTimeRule {
	before: TimeSpec;
	requireTags: string[];
}
export interface SmartRules {
	includeTags: string[];
	includeMode: TagMatch;
	excludeTags: string[];
	targetDurationMs?: number;
	maxItemMs?: number;
	noRepeatNights: number;
	timeRules: SmartTimeRule[];
	order: SmartOrder;
	pinnedFirst: PlaylistItem[];
	pinnedLast: PlaylistItem[];
	interleave: PlaylistItem[];
	interleaveEvery: number;
}

export interface TagDef {
	name: string;
	color?: string;
}

/** GET /playlists/:id/preview (F18). */
export interface SmartPreview {
	items: PlaylistItem[];
	totalMs: number;
	notes: string[];
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
	/** Start the intro early so the first song begins exactly at `start` (F4). */
	startExact?: boolean;
}

/** A daily window (wraps midnight when `to` is before `from`). */
export interface TimeWindow {
	from: TimeSpec;
	to: TimeSpec;
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
	/** How much later the audience hears the sound than it leaves the leader (FM, HDMI,
	 * Bluetooth, distance ≈ 3 ms per metre); every controller's lights are delayed by this
	 * much. Negative = lights earlier. Range −500…2000 ms. Set with "Sync lights to sound". */
	outputDelayMs?: number;
	/** Last automatic / manual calibration (F1). */
	lastCalibration?: AudioCalibration;
}
export interface AudioCalibration {
	measuredAt: string;
	method: 'phone' | 'manual';
	residualMs: number;
	spreadMs: number;
	matches: number;
	appliedDelayMs: number;
	device?: string;
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
	/** FM station visitors tune to, e.g. "88.7 FM" (shown on the request page and yard sign). */
	radioFrequency?: string;
	/** Internet address of the request page (e.g. through a tunnel); QR codes use it when set. */
	publicUrl?: string;
	/** Most requests one visitor (address) may make per hour; 0 = no limit (a burst limit of
	 *  3 per 10 minutes always applies). */
	perVisitorPerHour: number;
	/** Most requests from everyone together per hour; 0 = no limit. */
	maxPerHour: number;
}
export type TtsMode = 'auto' | 'device' | 'browser';
export interface TriggerAction {
	type: 'playPlaylist' | 'playSequence' | 'stop' | 'effect' | 'surprise';
	ref?: string;
	/** Surprise (F20): props to draw on. */
	target?: Target;
	/** Surprise: length in ms. */
	durationMs?: number;
	/** Surprise: what `ref` names. */
	source?: 'sequence' | 'effect';
}
export type TriggerWhen = 'always' | 'showOnly' | 'idleOnly' | 'offOnly';
export interface Trigger {
	id: Id;
	name: string;
	kind: 'gpio' | 'http' | 'sensor';
	gpio?: number;
	action: TriggerAction;
	/** `kind: 'sensor'`: which sensor input (F20). */
	sensor?: SensorRef;
	/** Absent = 0 (no cooldown). */
	cooldownS?: number;
	/** Absent = 'always'. */
	when?: TriggerWhen;
	activeWindow?: TimeWindow;
	/** Absent / 0 = unlimited. */
	maxPerHour?: number;
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
	/** Most phones from one visitor address in line or playing at once; 0 = no limit. */
	maxQueuePerVisitor: number;
}

export interface ShowSettings {
	audio: AudioSettings;
	alerts: AlertSettings;
	mqtt: MqttSettings;
	requests: RequestSettings;
	tts: { mode: TtsMode };
	oled: { enabled: boolean };
	security: {
		passwordHash?: string;
		/** Extra host names the UI answers to (tunnel / own domain); `*.example.com` allowed. */
		allowedHosts?: string[];
		/** Reverse proxies on the network whose X-Forwarded-For is believed (IP or CIDR). */
		trustedProxies?: string[];
	};
	triggers: Trigger[];
	games: GameSettings;
	/** Display units. Absent = follow the viewer's locale (US → °F). Values are stored metric. */
	units?: UnitSettings;
	/** Pixel output options (every controller). */
	output?: {
		/** Experimental: all strings on a controller latch together ("bottom-aligned"). */
		latchAlign: boolean;
	};
	// Feature wave (always present from the daemon; optional so older payloads type-check).
	https?: HttpsSettings;
	reports?: ReportSettings;
	power?: PowerSettings;
	remote?: RemoteSettings;
	updates?: UpdateSettings;
	xlights?: XlightsSettings;
}

// ---------------------------------------------------------------- feature wave settings
/** F1: HTTPS listener (certificates live on disk, not here). */
export interface HttpsSettings {
	enabled: boolean;
	extraNames?: string[];
}
/** F11: nightly report. */
export interface ReportSettings {
	enabled: boolean;
	/** "HH:MM" local. */
	time: string;
	email: boolean;
	push: boolean;
	onlyWhenProblems: boolean;
	keepDays: number;
}
/** F12: power limiter and late-night dimming. */
export type LimiterMode = 'off' | 'warn' | 'limit';
export interface DimWindow {
	from: TimeSpec;
	to: TimeSpec;
	/** Percent. */
	brightness: number;
	days: Weekday[];
}
export interface PowerSettings {
	mode: LimiterMode;
	safety: number;
	globalAmps?: number;
	globalWatts?: number;
	dim: DimWindow[];
	maxBrightness: number;
}
export interface NodeOutputRef {
	nodeId: Id;
	output: number;
}
export interface PowerSupply {
	id: Id;
	name: string;
	volts: number;
	amps: number;
	receiverIds: Id[];
	directOutputs: NodeOutputRef[];
	sensor?: SensorRef;
}
/** F14: remote access. */
export interface RemoteSettings {
	publicListener: boolean;
	tailscale?: { enabled: boolean; serveAdmin: boolean; funnelPublic: boolean; dnsName?: string };
	cloudflare?: { mode: 'quick' | 'token'; publicHost?: string; adminHost?: string; tokenSet: boolean };
}
/** F15: updates. */
export type UpdateChannel = 'stable' | 'beta';
export type AutoUpdate = 'off' | 'notify' | 'install';
export interface UpdateSettings {
	channel: UpdateChannel;
	auto: AutoUpdate;
	window: { from: string; to: string; days: Weekday[] };
	avoidShowHours: number;
}
/** F16: xLights FPP Connect uploads. `passwordHash` is "" when set (write-only). */
export interface XlightsSettings {
	fppConnect: boolean;
	passwordHash?: string;
	addToPlaylists: boolean;
	watchFolder?: string;
}
/** F8: a season profile. */
export interface ShowProfile {
	id: Id;
	name: string;
	icon?: string;
	color?: string;
	dateRange?: DateRange;
	priority: number;
	schedule: Schedule;
	requestsPlaylistId?: Id;
	requestsMessage?: string;
	defaultDjVoice?: string;
	gamesEnabled?: boolean;
	power?: { dim: DimWindow[]; maxBrightness?: number };
	disabledPropIds?: Id[];
	tags?: string[];
}
/** F20: ESP32 sensor nodes. */
export type SensorInputKind = 'motion' | 'button' | 'beam' | 'contact' | 'current';
export interface SensorInput {
	id: string;
	name: string;
	pin: number;
	kind: SensorInputKind;
	activeLow: boolean;
	debounceMs: number;
	holdMs: number;
	/** `kind: 'current'` (INA219/INA226, `pin` = I²C address): shunt in milliohms. */
	shuntMilliohms?: number;
}
export interface SensorNode {
	id: Id;
	name: string;
	hw: string;
	location?: string;
	inputs: SensorInput[];
	adopted: boolean;
}
export interface SensorRef {
	sensorNodeId: Id;
	input: string;
}
export type TemperatureUnit = 'c' | 'f';
export interface UnitSettings {
	temperature: TemperatureUnit;
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
	/** Show file format (F15); absent in old payloads = 1. */
	formatVersion?: number;
	profiles?: ShowProfile[];
	activeProfileId?: Id;
	profileAutoSwitch?: boolean;
	powerSupplies?: PowerSupply[];
	tagDefs?: TagDef[];
	sensorNodes?: SensorNode[];
}

// ---------------------------------------------------------------- runtime / API
export interface SystemInfo {
	version: string;
	nodeId: Id;
	role: NodeRole | 'unconfigured';
	hostname: string;
	/** This controller's friendly name (setup wizard / adoption); falls back to the hostname. */
	name?: string;
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
	/** The hotspot has a password ("pixelplus" until the controller has been online once,
	 *  then a per-device one, also in PIXELPLUS-HOTSPOT.txt on the SD card). */
	hotspotSecured: boolean;
	/** The hotspot's current password (signed-in owner only). */
	hotspotPassword?: string | null;
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
	/** Two devices announce this id from different addresses (cloned SD card or an impostor). */
	duplicate?: boolean;
	/** A show leader whose owner chose "Join another show" (adopting replaces its show). */
	joining?: boolean;
	/** F10: a controller that was replaced ("Replace with…") and is back: offer "Release it". */
	retired?: { replacedAt: string; name: string } | null;
}

/** "Join another show" / "Allow a new leader" (POST /system/join-show). */
export interface JoinWindow {
	open: boolean;
	secondsLeft: number;
	leaderAddress?: string | null;
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
	/** Power limiter (F12), while it is not off. */
	power?: { limiting: boolean; minScale: number };
}

/** Body of POST /player/play. Manual play outside a show window plays a playlist once
 *  (its repeat is ignored) unless `loopUntilStopped`; started inside a window it ends with
 *  the window (the entry's end behaviour), unless `loopUntilStopped`. */
export interface PlayRequest {
	playlistId?: Id;
	sequenceId?: Id;
	djClipId?: Id;
	effectId?: Id;
	mediaId?: Id;
	startIndex?: number;
	/** "Loop until I stop". */
	loopUntilStopped?: boolean;
}

/** How well a follower keeps time with its leader (all times in ms). */
export interface SyncQuality {
	/** Error bound of the leader-clock estimate (half the best round trip + fit noise). */
	offsetErrorMs: number;
	/** Residual noise of the clock fit. */
	jitterMs: number;
	/** Drift of the leader's clock against this controller's, ppm. */
	driftPpm: number;
	/** Round trip: best, median, 95th percentile over the last 90 s. */
	rttMs: number;
	rttP50Ms: number;
	rttP95Ms: number;
	/** Clock probes without an answer, percent. */
	lossPct: number;
	samples: number;
	/** How far the player is from the leader's timeline (smoothed). */
	timelineErrorMs?: number;
	/** Pixel refresh of this controller (frame changes land within ±half a refresh). */
	refreshHz?: number;
	kernelTimestamps: boolean;
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
	role?: NodeRole;
	/** Followers (protocol 2): timing quality. */
	sync?: SyncQuality | null;
	/** Wi-Fi power saving on (bad for sync); null when unknown / wired. */
	wifiPowerSave?: boolean | null;
	/** Cluster protocol version the node runs. */
	protocol?: number;
	problem?: string | null;
	version?: string | null;
	/** Power limiter activity from the node's report (F12); absent while its limiter is off
	 *  or the node is offline. */
	limiter?: LimiterReport;
}

/** What a node's power limiter did (F12, follower beacon report). */
export interface LimiterReport {
	/** Budget groups currently scaling. */
	activeGroups: Id[];
	/** Lowest scale applied in the last report period (1 = none). */
	minScale: number;
	/** Seconds spent limiting since the daemon started. */
	secondsLimited: number;
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

export type TestMode =
	'solid' | 'chase' | 'rgbCycle' | 'countPixels' | 'walk' | 'effect' | 'mapCode' | 'identify' | 'calibration';
export interface TestRequest {
	mode: TestMode;
	color?: string;
	target: { nodeId?: Id; output?: number; propIds?: Id[]; groupIds?: Id[]; all?: boolean };
	effect?: EffectPreset;
	/** mapCode (F6/F7). */
	map?: MapPlan;
	mapRunId?: string;
	/** identify (F9). */
	identify?: IdentifyLight[];
	/** calibration v2 (F1). */
	cal?: { seed: number; v: number };
}

// ---------------------------------------------------------------- feature wave runtime
/** F6/F7 camera mapping plan (pixelplus-core::mapcode). */
export interface MapTarget {
	nodeId: Id;
	output: number;
	maxPixels: number;
}
export interface MapPlan {
	seed: number;
	bitMs: number;
	level: number;
	passes: number;
	/** bit 0 = phase A, bit 1 = phase B. */
	phases: number;
	targets: MapTarget[];
	pixelBits: number;
	startPosMs: number;
}
/** POST /mapping/runs → */
export interface MappingRunStart {
	runId: string;
	plan: MapPlan;
	schedule: { preambleMs: number; phaseAms: number; phaseBms: number; totalMs: number };
	codebook: number[][];
}
export interface MappingProposal {
	id: string;
	kind: 'swap' | 'reverse' | 'pixelCount' | 'notSeen' | 'layout';
	propId?: Id;
	message: string;
	/** Kind-specific details. */
	data?: Record<string, unknown>;
}
export interface MappingRun {
	id: string;
	startedAt: string;
	scope: { all?: boolean; nodeId?: Id; propIds?: Id[] };
	plan: MapPlan;
	targets: { k: number; nodeId: Id; output: number; label: string }[];
	results?: {
		detected: { k: number; pixels: [number, number, number, number][] }[];
		proposals: MappingProposal[];
	};
	appliedSnapshotId?: string;
}
/** F9 receiver wizard. */
export interface IdentifyLight {
	nodeId: Id;
	output: number;
	color: string;
	blinks: number;
}
export interface JackCandidate {
	jack: number;
	color: string;
	blinks: number;
}
/** F7 manual pixel-count search step. */
export interface PixelCountStep {
	session: string;
	step?: { litUntil: number; ask: string };
	count?: number;
}
/** F1 TLS / secure context. */
export interface TlsStatus {
	enabled: boolean;
	port: number;
	caFingerprint: string;
	caSubject: string;
	leafNames: string[];
	leafNotAfter: string;
	urls: { lan: string[]; tailscale?: string; tunnel?: string };
	secureNow: boolean;
	/** WS1 additions: listener state (this node serves HTTPS now), bind error, node role. */
	active?: boolean;
	listening?: boolean;
	error?: string | null;
	role?: 'leader' | 'follower' | 'unconfigured';
	caCreatedAt?: string | null;
	caNotAfter?: string | null;
	leafIssuedAt?: string | null;
	/** `https.extraNames` the local CA can't vouch for (public domains). */
	rejectedNames?: string[];
}
/** POST /player/calibration {on, pattern: 'v2'} → (see pixelplus_core::calpattern) */
export interface CalibrationPattern {
	seed: number;
	eventsMs: number[];
	flashMs: number;
	startsInMs: number;
	/** WS1 additions (optional; lib/sensing/schedule.ts rebuilds them from the seed). */
	v?: number;
	windowMs?: number;
	leadInMs?: number;
	chirp?: { ms: number; f0Hz: number; f1Hz: number; rampMs: number };
}
export interface CalibrationResultBody {
	residualMs: number;
	spreadMs: number;
	matches: number;
	device?: string;
	apply: boolean;
}
/** F2 auto light show. */
export interface AutoshowStyle {
	id: string;
	name: string;
	description: string;
}
/** WS `job` (F2/F3). */
export interface JobStatus {
	id: string;
	kind: 'analysis' | 'autoshow' | 'preview';
	pct: number;
	state: 'queued' | 'running' | 'done' | 'failed';
	result?: { sequenceId?: Id; message?: string };
}
/** F3 preview header (GET /sequences/:id/preview). */
export interface PreviewHeader {
	v: number;
	seqId: Id;
	frameMs: number;
	frameCount: number;
	props: { id: Id; n: number }[];
	blockFrames: number;
	blocks: { offset: number; len: number }[];
}
/** F11 reports. */
export type ReportStatus = 'ok' | 'warn' | 'fail';
/** `GET /reports` entry. */
export interface ReportSummary {
	date: string;
	status: ReportStatus;
	headline: string;
	itemsPlayed: number;
	requests: number;
	/** Problem occurrences (sum of `problems[].count`). */
	problems: number;
	runtimeMin: number;
	/** Hottest board of the night (absent without temperature readings). */
	tempMaxC?: number;
}
/** One charted report series (`[unix ms, value]` points). */
export interface ReportSeries {
	nodeId: Id;
	name: string;
	points: [number, number][];
}
export interface NightReport {
	date: string;
	status: ReportStatus;
	headline: string;
	shows: { entryId: Id; name: string; startedAt: string; runtimeMin: number }[];
	itemsPlayed: number;
	requests: number;
	topRequests: { sequenceId: Id; name: string; count: number }[];
	problems: { level: 'warn' | 'error'; code: string; message: string; count: number }[];
	nodes: {
		nodeId: Id;
		name: string;
		tempMinC?: number;
		tempMaxC?: number;
		voltsMin?: number;
		offlineMin: number;
		syncP50Ms?: number;
		syncP95Ms?: number;
	}[];
	limiter: { nodeId: Id; port: number; seconds: number }[];
	suspectPixels: { propId: Id; name: string; pixels: number[] }[];
	diskFreePct?: number;
	updates: string[];
	backupAgeDays?: number;
	/** RFC 3339. */
	generatedAt: string;
	/** The part of the night covered, RFC 3339 `[from, to)`. */
	window: { from: string; to: string };
	/** Total show-window minutes. */
	runtimeMin: number;
	games: number;
	gameMinutes: number;
	/** Triggers / sensor surprises fired. */
	triggers: number;
	/** Daemon starts during the night. */
	restarts: number;
	/** Active season, if any. */
	season?: string;
	series: { tempC: ReportSeries[]; syncMs: ReportSeries[] };
	/** What happened when it was sent ("Email sent to …"). */
	delivery?: string[];
}
/** F11 show journal (GET /journal). */
export interface JournalRecord {
	ts: string;
	ev: string;
	[field: string]: unknown;
}
/** F12 live power (GET /power/live, WS `power`). */
export interface PowerLive {
	nodes: { nodeId: Id; groups: { id: string; amps: number; budget: number; scale: number }[] }[];
}
/** F14 remote access status. */
export interface RemoteStatus {
	tailscale: {
		installed: boolean;
		state: string;
		dnsName?: string;
		httpsOk: boolean;
		funnel: boolean;
		/** `tailscale serve` publishes the admin pages to the tailnet (WS5). */
		serve?: boolean;
		/** Open to connect this controller to a tailnet. */
		loginUrl?: string;
		ips?: string[];
	};
	cloudflare: {
		installed: boolean;
		running: boolean;
		mode?: 'quick' | 'token';
		urls: string[];
		publicHost?: string;
		adminHost?: string;
		tokenSet?: boolean;
	};
	publicListener: boolean;
	publicPort?: number;
	/** A web UI password is set (required before exposing the admin pages). */
	passwordSet?: boolean;
	/** Remote access can be set up from here (PixelPlus Pi image / package, not Docker). */
	canManage?: boolean;
	message?: string;
}
/** F20 sensor nodes. */
export interface DiscoveredSensorNode {
	id: string;
	name: string;
	hw: string;
	ver: string;
	ip?: string;
	adoptedBy?: string | null;
	inputs: string[];
}
/** WS `sensorInput`. */
export interface SensorInputEvent {
	sensorNodeId: Id;
	input: string;
	state: number;
	at: string;
}
/** F8 profile switch preview. */
export interface ProfileSwitchDiff {
	lines: string[];
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
	/** FM station, when the owner set one. */
	radioFrequency?: string | null;
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

/** WebSocket messages `{type, data}` by type (ARCHITECTURE §8.1); `app.onMessage(type, cb)`. */
export interface WsPayloads {
	status: PlayerStatus;
	show: { version: number };
	nodes: NodeStatus[];
	sensors: Sensor[];
	log: LogLine;
	toast: ToastMsg;
	helper: HelperStatus;
	system: unknown;
	/** F2/F3 background jobs. */
	job: JobStatus;
	/** F12 live power, every second while playing. */
	power: PowerLive;
	/** F6 mapping run progress. */
	mapping: { runId: string; state: 'running' | 'done' | 'stopped'; pct: number };
	/** F20 a sensor input changed. */
	sensorInput: SensorInputEvent;
	/** F15 cluster update progress (every state change). */
	updateJob: UpdateRun;
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
	// F15 (cluster updates, WS5):
	nodes?: {
		id: Id;
		name?: string;
		version: string;
		proto?: number;
		canApply: boolean;
		online?: boolean;
		arch?: string;
		/** idle | staging | staged | committing | rollingBack | failed */
		phase?: string;
	}[];
	history?: {
		at: string;
		from: string;
		to: string;
		ok: boolean;
		scope: 'cluster' | 'this';
		message?: string;
		nodes?: number;
	}[];
	/** Signed over-the-air updates are used here (else apt). */
	ota?: boolean;
	/** What keeps "Update everything" from starting right now. */
	problems?: string[];
	/** The current / last cluster update. */
	run?: UpdateRun | null;
	/** The version before the last update ("Roll back to …"). */
	previous?: string;
}

/** F15: one cluster update / rollback job (`GET /system/update` `run`, WS `updateJob`). */
export interface UpdateRun {
	id: string;
	kind: 'update' | 'rollback';
	scope: 'cluster' | 'this';
	version: string;
	from: string;
	phase:
		'staging' | 'committingFollowers' | 'committingLeader' | 'rollingBack' | 'done' | 'rolledBack' | 'failed';
	nodes: {
		id: Id;
		name: string;
		isSelf: boolean;
		arch: string;
		from: string;
		phase:
			'pending' | 'staging' | 'staged' | 'committing' | 'healthy' | 'failed' | 'rollingBack' | 'rolledBack';
		committed?: boolean;
		message?: string;
	}[];
	startedAt: string;
	finishedAt?: string;
	message?: string;
	auto?: boolean;
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

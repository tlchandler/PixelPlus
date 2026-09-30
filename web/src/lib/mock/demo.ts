// A realistic demo show used by the in-browser mock backend.
import type {
	BoardKind,
	DjVoice,
	EffectPreset,
	Media,
	Node,
	OutputConfig,
	Playlist,
	Prop,
	PropKind,
	PropLayout,
	Show
} from '$lib/api/types';
import { outputLabel } from '$lib/util/boards';
import { DEFAULT_EFFECT_SCHEMA, defaultParams } from '$lib/effects/render';

function outputs(board: BoardKind, n: number): OutputConfig[] {
	return Array.from({ length: n }, (_, i) => ({
		index: i + 1,
		label: outputLabel(board, i + 1),
		pixelType: 'ws2811' as const,
		colorOrder: 'RGB' as const,
		brightness: 100,
		gamma: 1,
		enabled: true
	}));
}

export const MAIN = 'nmain00001';
export const GARAGE = 'ngarage001';

function prop(
	id: string,
	name: string,
	kind: PropKind,
	pixelCount: number,
	layout: PropLayout,
	extra: Partial<Prop> = {}
): Prop {
	return {
		id,
		name,
		kind,
		pixelCount,
		xlightsModel: name,
		channelStart: 0,
		channelsPerPixel: 3,
		segments: [],
		groupIds: [],
		layout,
		...extra
	};
}

function linePoints(n: number, x0: number, y0: number, x1: number, y1: number): [number, number][] {
	return Array.from({ length: n }, (_, i) => {
		const t = n === 1 ? 0 : i / (n - 1);
		return [x0 + (x1 - x0) * t, y0 + (y1 - y0) * t];
	});
}

function voice(
	id: string,
	name: string,
	description: string,
	blend: Record<string, number>,
	defaultEnergy = 0.4
): DjVoice {
	return {
		id,
		name,
		description,
		blend,
		speed: 1,
		lang: 'en-us',
		defaultEnergy,
		energy: { pitch: 1.5, range: 1.5, speed: 1, stretch: 1, boost: 3, lift: 3, ceiling: 3, maxLift: 9 }
	};
}

export function buildDemoShow(): Show {
	const nodes: Node[] = [
		{
			id: MAIN,
			name: 'Main Controller',
			hostname: 'pixelplus-main',
			role: 'leader',
			board: 'difftxlarge',
			boardRev: 'A',
			piModel: 'Raspberry Pi 4 Model B Rev 1.5',
			outputs: outputs('difftxlarge', 60),
			adopted: true
		},
		{
			id: GARAGE,
			name: 'Garage',
			hostname: 'pixelplus-garage',
			role: 'follower',
			board: 'difftx',
			boardRev: 'D',
			piModel: 'Raspberry Pi Zero 2 W Rev 1.0',
			outputs: outputs('difftx', 4),
			adopted: true
		}
	];
	nodes[1].outputs[1].colorOrder = 'GRB';

	const receivers = [
		{
			id: 'rxfront001',
			name: 'Front Yard',
			kind: 'diffrx' as const,
			nodeId: MAIN,
			jack: 1,
			location: 'Behind the hedge',
			fuseAmps: 6
		},
		{
			id: 'rxporch001',
			name: 'Porch',
			kind: 'diffrx' as const,
			nodeId: MAIN,
			jack: 2,
			location: 'Porch ceiling',
			fuseAmps: 6
		},
		{
			id: 'rxroof0001',
			name: 'Roofline',
			kind: 'diffrx' as const,
			nodeId: MAIN,
			jack: 3,
			location: 'Attic',
			fuseAmps: 6
		},
		{
			id: 'rxmatrix01',
			name: 'Matrix',
			kind: 'diffrx' as const,
			nodeId: MAIN,
			jack: 4,
			location: 'Matrix frame',
			fuseAmps: 6
		},
		{
			id: 'rxtree0001',
			name: 'Mega Tree',
			kind: 'diffsmart-rx' as const,
			nodeId: MAIN,
			jack: 6,
			location: 'Tree base',
			fuseAmps: 6
		},
		{
			id: 'rxdrive001',
			name: 'Driveway',
			kind: 'diffrx' as const,
			nodeId: GARAGE,
			jack: 1,
			location: 'Garage wall',
			fuseAmps: 6
		}
	];

	const props: Prop[] = [];
	const add = (p: Prop, segs: [string, number, number, number?][] = []) => {
		let off = 0;
		for (const [nodeId, output, start, count] of segs) {
			const c = count ?? p.pixelCount - off;
			p.segments.push({
				nodeId,
				output,
				startPixel: start,
				pixelCount: c,
				propOffset: off,
				reverse: false,
				nullPixels: 0
			});
			off += c;
		}
		props.push(p);
		return p;
	};

	// Mega tree + star (left)
	add(
		prop(
			'ptree00001',
			'Mega Tree',
			'tree',
			800,
			{ x: 50, y: 190, w: 200, h: 380, rotation: 0 },
			{ color: '#3fcf8e' }
		),
		[
			[MAIN, 21, 0, 200],
			[MAIN, 22, 0, 200],
			[MAIN, 23, 0, 200],
			[MAIN, 24, 0, 200]
		]
	);
	add(
		prop(
			'pstar00001',
			'Tree Topper Star',
			'star',
			60,
			{ x: 115, y: 120, w: 70, h: 70, rotation: 0 },
			{ color: '#f5d547' }
		),
		[[MAIN, 8, 0]]
	);
	// House roofline
	add(
		prop('proofl0001', 'Roofline Left', 'line', 150, {
			x: 320,
			y: 120,
			w: 280,
			h: 150,
			rotation: 0,
			points: linePoints(150, 0, 1, 1, 0)
		}),
		[[MAIN, 9, 0]]
	);
	add(
		prop('proofr0001', 'Roofline Right', 'line', 150, {
			x: 600,
			y: 120,
			w: 280,
			h: 150,
			rotation: 0,
			points: linePoints(150, 0, 0, 1, 1)
		}),
		[[MAIN, 10, 0]]
	);
	add(prop('peave00001', 'Roofline Eave', 'line', 200, { x: 320, y: 270, w: 560, h: 6, rotation: 0 }), [
		[MAIN, 11, 0]
	]);
	add(
		prop(
			'picicle001',
			'Icicles',
			'icicles',
			300,
			{ x: 320, y: 278, w: 560, h: 40, rotation: 0 },
			{ color: '#9fd8ff' }
		),
		[[MAIN, 12, 0]]
	);
	// Windows + porch
	add(prop('pwinl00001', 'Window Left', 'window', 60, { x: 370, y: 330, w: 90, h: 80, rotation: 0 }), [
		[MAIN, 5, 0]
	]);
	add(prop('pwinr00001', 'Window Right', 'window', 60, { x: 740, y: 330, w: 90, h: 80, rotation: 0 }), [
		[MAIN, 6, 0]
	]);
	add(
		prop('pcoll00001', 'Porch Column Left', 'line', 50, {
			x: 540,
			y: 330,
			w: 6,
			h: 150,
			rotation: 0,
			points: linePoints(50, 0.5, 1, 0.5, 0)
		}),
		[[MAIN, 7, 0]]
	);
	add(
		prop('pcolr00001', 'Porch Column Right', 'line', 50, {
			x: 654,
			y: 330,
			w: 6,
			h: 150,
			rotation: 0,
			points: linePoints(50, 0.5, 0, 0.5, 1)
		}),
		[[MAIN, 7, 50]]
	);
	// Arches: 3 per port on Front Yard receiver ports 1 & 2
	for (let i = 0; i < 6; i++) {
		const p = prop(`parch0000${i + 1}`, `Arch ${i + 1}`, 'arch', 50, {
			x: 300 + i * 100,
			y: 520,
			w: 84,
			h: 70,
			rotation: 0
		});
		add(p, [[MAIN, 1 + Math.floor(i / 3), (i % 3) * 50]]);
	}
	// Singing matrix
	const mw = 80,
		mh = 40;
	add(
		prop(
			'pmatrix001',
			'Singing Matrix',
			'matrix',
			mw * mh,
			{ x: 950, y: 250, w: 220, h: 110, rotation: 0 },
			{
				matrix: { width: mw, height: mh, pixelMap: Array.from({ length: mw * mh }, (_, i) => i) },
				color: '#a88bfa',
				maxMilliampsPerPixel: 36
			}
		),
		[
			[MAIN, 13, 0, 800],
			[MAIN, 14, 0, 800],
			[MAIN, 15, 0, 800],
			[MAIN, 16, 0, 800]
		]
	);
	// Candy canes on the Garage follower (Driveway receiver ports 1 & 2)
	for (let i = 0; i < 8; i++) {
		const p = prop(`pcane0000${i + 1}`, `Candy Cane ${i + 1}`, 'candycane', 25, {
			x: 960 + i * 28,
			y: 480,
			w: 22,
			h: 70,
			rotation: 0
		});
		add(p, [[GARAGE, 1 + Math.floor(i / 4), (i % 4) * 25]]);
	}
	add(
		prop(
			'pspin00001',
			'Spinner',
			'spinner',
			144,
			{ x: 1010, y: 90, w: 110, h: 110, rotation: 0 },
			{ color: '#5b9dff' }
		),
		[[GARAGE, 3, 0]]
	);
	for (let i = 0; i < 4; i++)
		add(
			prop(`pmini0000${i + 1}`, `Mini Tree ${i + 1}`, 'tree', 50, {
				x: 40 + i * 60,
				y: 600,
				w: 44,
				h: 64,
				rotation: 0
			}),
			[[GARAGE, 4, i * 50]]
		);
	add(
		prop(
			'psnowflak1',
			'Snowflake',
			'star',
			40,
			{ x: 1160, y: 110, w: 60, h: 60, rotation: 0 },
			{ notes: 'New this year' }
		)
	);

	// channel starts (xLights order)
	let ch = 0;
	for (const p of props) {
		p.channelStart = ch;
		ch += p.pixelCount * 3;
	}

	const groups = [
		{
			id: 'garches001',
			name: 'Arches',
			propIds: props.filter((p) => p.kind === 'arch').map((p) => p.id),
			color: '#f5a524'
		},
		{
			id: 'gcanes0001',
			name: 'Candy Canes',
			propIds: props.filter((p) => p.kind === 'candycane').map((p) => p.id),
			color: '#f2555a'
		},
		{
			id: 'ghouse0001',
			name: 'House',
			propIds: [
				'proofl0001',
				'proofr0001',
				'peave00001',
				'picicle001',
				'pwinl00001',
				'pwinr00001',
				'pcoll00001',
				'pcolr00001'
			],
			color: '#5b9dff'
		},
		{
			id: 'gtrees0001',
			name: 'Trees',
			propIds: [
				'ptree00001',
				'pstar00001',
				...props.filter((p) => p.name.startsWith('Mini Tree')).map((p) => p.id)
			],
			color: '#3fcf8e'
		},
		{
			id: 'gyard00001',
			name: 'Yard',
			propIds: props.filter((p) => ['arch', 'candycane'].includes(p.kind)).map((p) => p.id),
			color: '#a88bfa'
		}
	];
	for (const g of groups) for (const pid of g.propIds) props.find((p) => p.id === pid)?.groupIds.push(g.id);

	const songs: [string, string, number, number][] = [
		['wizards', 'Wizards in Winter', 185000, -9.8],
		['carol', 'Carol of the Bells', 156000, -11.2],
		['russian', "Mad Russian's Christmas", 277000, -8.9],
		['sarajevo', 'Christmas Eve / Sarajevo 12/24', 204000, -10.4],
		['allwant', 'All I Want for Christmas Is You', 241000, -7.6],
		['letitgo', 'Let It Go', 224000, -12.8],
		['jbrock', 'Jingle Bell Rock', 123000, -13.5],
		['feliz', 'Feliz Navidad', 181000, -14.1]
	];
	const media: Media[] = songs.map(([k, name, dur, lufs]) => ({
		id: `m${k}`.padEnd(10, '0').slice(0, 10),
		name,
		kind: 'song' as const,
		file: `media/${k}.mp3`,
		durationMs: dur,
		loudnessLufs: lufs,
		gainDb: +(-14 - lufs).toFixed(1)
	}));
	const sequences = songs.map(([k, name, dur], i) => ({
		id: `s${k}`.padEnd(10, '0').slice(0, 10),
		name,
		file: `sequences/s${k}.fseq`,
		durationMs: dur,
		frameMs: i % 3 === 0 ? 25 : 50,
		channelCount: ch,
		mediaId: i === 6 ? undefined : media[i].id,
		xlightsName: `${name.replace(/[^A-Za-z0-9]+/g, '_')}.fseq`,
		hash: (k + '9f3ab2c7d1e5').padEnd(64, '0')
	}));
	media.push(
		{
			id: 'mdjwelcom1',
			name: 'DJ — Welcome',
			kind: 'dj',
			file: 'media/dj-welcome.mp3',
			durationMs: 14000,
			loudnessLufs: -15.2,
			gainDb: 1.2
		},
		{
			id: 'mdjradio01',
			name: 'DJ — Tune to 88.3',
			kind: 'dj',
			file: 'media/dj-radio.mp3',
			durationMs: 9000,
			loudnessLufs: -14.8,
			gainDb: 0.8
		},
		{
			id: 'mbedjazz01',
			name: 'Jazzy music bed',
			kind: 'sfx',
			file: 'media/bed-jazz.mp3',
			durationMs: 60000,
			loudnessLufs: -20
		},
		{
			id: 'msleigh001',
			name: 'Sleigh bells',
			kind: 'sfx',
			file: 'media/sleigh.mp3',
			durationMs: 4000,
			loudnessLufs: -18
		}
	);

	const djVoices = [
		voice(
			'nick',
			'Nick',
			'Warm, upbeat, classic radio baritone.',
			{ am_echo: 0.3, am_fenrir: 0.3, am_puck: 0.4 },
			0.4
		),
		voice('holly', 'Holly', 'Bright, friendly and energetic co-host.', { af_heart: 0.5, af_kore: 0.5 }, 0.4),
		voice('santa', 'Santa', 'Deep and jolly. Ho ho ho.', { am_santa: 0.7, am_onyx: 0.3 }, 0.3)
	];

	const djClips = [
		{
			id: 'djwelcome1',
			name: 'Welcome to the show',
			dynamic: false,
			speed: 1,
			mediaId: 'mdjwelcom1',
			musicBedMediaId: 'mbedjazz01',
			lines: [
				{
					voice: 'nick',
					text: 'Good evening and welcome to the Chandler Lights show!',
					pauseMs: 300,
					energy: 1
				},
				{
					voice: 'holly',
					text: 'Tune your radio to eighty-eight point three FM, and please keep the driveway clear.',
					pauseMs: 250,
					energy: 0.4
				},
				{ voice: 'nick', text: 'Grab some cocoa, sit back — here we go!', pauseMs: 0, energy: 1.5 }
			]
		},
		{
			id: 'djupnext01',
			name: 'Up next',
			dynamic: true,
			speed: 1,
			lines: [
				{
					voice: 'holly',
					text: 'That was {prevSong}. It is {time}, and there are {daysUntilChristmas} days until Christmas!',
					pauseMs: 250,
					energy: 0.4
				},
				{ voice: 'nick', text: 'Up next: {nextSong}!', pauseMs: 0, energy: 1 }
			]
		},
		{
			id: 'djradio001',
			name: 'Radio reminder',
			dynamic: false,
			speed: 1.05,
			mediaId: 'mdjradio01',
			lines: [
				{
					voice: 'holly',
					text: 'Reminder: the music is on 88.3 FM. Please dim your headlights!',
					pauseMs: 0,
					energy: 0.4
				}
			]
		},
		{
			id: 'djgoodnit1',
			name: 'Good night',
			dynamic: false,
			speed: 0.95,
			lines: [
				{
					voice: 'santa',
					text: 'Ho ho ho! That is all for tonight. Merry Christmas to all…',
					pauseMs: 300,
					energy: 0.3
				},
				{ voice: 'holly', text: 'And to all a good night!', pauseMs: 0, energy: 0.4 }
			]
		}
	];

	const fx = (
		id: string,
		name: string,
		effect: EffectPreset['effect'],
		params: Record<string, unknown> = {}
	): EffectPreset => ({
		id,
		name,
		effect,
		params: { ...defaultParams(DEFAULT_EFFECT_SCHEMA[effect]), ...(params as any) },
		target: { all: true }
	});
	const effects = [
		fx('ewarmwht01', 'Warm White Glow', 'twinkle', {
			colors: ['#ffc98a', '#ffb46b'],
			density: 0.6,
			speed: 0.5,
			glow: 0.3
		}),
		fx('ecandy0001', 'Candy Cane Stripes', 'candycane', {}),
		fx('erainbow01', 'Rainbow Flow', 'rainbow', { speed: 0.3, mode: 'across' }),
		fx('esnow00001', 'Gentle Snowfall', 'snow', {}),
		fx('efire00001', 'Yule Fire', 'fire', {}),
		fx('ewash00001', 'Classic Color Wash', 'colorwash', {
			colors: ['#ff2a2a', '#1fbf4f', '#ffd700'],
			speed: 0.1,
			spread: 0.5
		}),
		fx('ewave00001', 'Northern Lights', 'wave', {
			colors: ['#1ee3a0', '#5b2bff', '#0a2a6a'],
			speed: 0.15,
			wavelength: 0.8
		}),
		fx('emeteor001', 'Icicle Drip', 'meteor', { colors: ['#bfe6ff'], speed: 40, tailLength: 20, count: 2 }),
		fx('esparkle01', 'Blue Sparkle', 'sparkle', {}),
		fx('ebreathe01', 'Red & Green Breathe', 'breathe', {})
	];

	const seq = (i: number) => sequences[i].id;
	const it = (id: string, rest: any) => ({ id, ...rest });
	const playlists: Playlist[] = [
		{
			id: 'plmain0001',
			name: 'Main Show',
			shuffle: false,
			repeat: true,
			crossfadeMs: 0,
			intro: [it('i01', { type: 'dj', djClipId: 'djwelcome1' })],
			items: [
				it('i02', { type: 'sequence', sequenceId: seq(0) }),
				it('i03', { type: 'sequence', sequenceId: seq(1) }),
				it('i04', { type: 'dj', djClipId: 'djupnext01' }),
				it('i05', { type: 'sequence', sequenceId: seq(2) }),
				it('i06', { type: 'command', command: 'games.invite', args: {} }),
				it('i07', { type: 'sequence', sequenceId: seq(3) }),
				it('i08', { type: 'sequence', sequenceId: seq(4) }),
				it('i09', { type: 'dj', djClipId: 'djradio001' }),
				it('i10', { type: 'sequence', sequenceId: seq(5) }),
				it('i11', { type: 'effect', effectId: 'ewarmwht01', durationMs: 30000 })
			],
			outro: [it('i12', { type: 'dj', djClipId: 'djgoodnit1' })]
		},
		{
			id: 'plkids0001',
			name: 'Kids Hour',
			shuffle: true,
			repeat: true,
			crossfadeMs: 1500,
			intro: [],
			items: [
				it('k01', { type: 'sequence', sequenceId: seq(6) }),
				it('k02', { type: 'sequence', sequenceId: seq(7) }),
				it('k03', { type: 'sequence', sequenceId: seq(5) }),
				it('k04', { type: 'pause', durationMs: 10000 })
			],
			outro: []
		},
		{
			id: 'plxmaseve1',
			name: 'Christmas Eve Spectacular',
			shuffle: false,
			repeat: true,
			crossfadeMs: 0,
			intro: [it('x01', { type: 'dj', djClipId: 'djwelcome1' })],
			items: sequences.map((s, i) => it(`x${i + 10}`, { type: 'sequence', sequenceId: s.id })),
			outro: [it('x99', { type: 'dj', djClipId: 'djgoodnit1' })]
		}
	];

	const show: Show = {
		version: 42,
		name: 'Chandler Lights',
		nodes,
		receivers,
		props,
		propGroups: groups,
		sequences,
		media,
		djClips,
		djVoices,
		pronunciations: [
			{ word: 'Noel', say: '/noʊˈɛl/' },
			{ word: 'Chandler', say: 'Chand-ler' },
			{ word: 'FM', say: 'eff em' }
		],
		effects,
		playlists,
		schedule: {
			enabled: true,
			location: { lat: 41.8781, lon: -87.6298, timezone: 'America/Chicago', label: 'Chicago, IL' },
			idleEffectId: 'ewarmwht01',
			volumeCurfew: { time: { kind: 'clock', time: '21:30' }, volume: 45 },
			entries: [
				{
					id: 'scweeknt01',
					name: 'Weeknights',
					enabled: true,
					playlistId: 'plmain0001',
					days: ['mon', 'tue', 'wed', 'thu', 'sun'],
					start: { kind: 'sunset', offsetMin: 15 },
					end: { kind: 'clock', time: '22:00' },
					priority: 0,
					endBehavior: 'finishSong'
				},
				{
					id: 'scweekend1',
					name: 'Weekend nights',
					enabled: true,
					playlistId: 'plmain0001',
					days: ['fri', 'sat'],
					start: { kind: 'sunset', offsetMin: 0 },
					end: { kind: 'clock', time: '23:00' },
					priority: 1,
					endBehavior: 'finishSong'
				},
				{
					id: 'sckids0001',
					name: 'Kids Hour',
					enabled: true,
					playlistId: 'plkids0001',
					days: ['sat', 'sun'],
					start: { kind: 'sunset', offsetMin: -30 },
					end: { kind: 'sunset', offsetMin: 30 },
					priority: 2,
					endBehavior: 'fadeOut'
				},
				{
					id: 'scxmaseve1',
					name: 'Christmas Eve',
					enabled: true,
					playlistId: 'plxmaseve1',
					days: ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'],
					dateRange: { start: '12-24', end: '12-24' },
					start: { kind: 'sunset', offsetMin: 0 },
					end: { kind: 'clock', time: '23:59' },
					priority: 10,
					endBehavior: 'finishSong'
				}
			]
		},
		settings: {
			audio: { device: 'hw:CARD=Headphones', volume: 72, normalize: true, targetLufs: -14 },
			alerts: {
				ntfy: { server: 'https://ntfy.sh', topic: 'chandler-lights-alerts' },
				rules: { tempC: 65, voltageMin: 11.2, followerOffline: true, showFailure: true }
			},
			mqtt: {
				enabled: false,
				host: 'homeassistant.local',
				port: 1883,
				baseTopic: 'pixelplus',
				homeAssistantDiscovery: true
			},
			requests: {
				enabled: true,
				maxQueue: 5,
				playlistId: 'plmain0001',
				title: 'Request a song',
				message: 'Pick a song and it plays next. Merry Christmas from the Chandlers!',
				radioFrequency: '88.3 FM'
			},
			tts: { mode: 'auto' },
			oled: { enabled: true },
			security: {},
			triggers: [
				{
					id: 'trbutton01',
					name: 'Mailbox button',
					kind: 'gpio',
					gpio: 17,
					action: { type: 'playPlaylist', ref: 'plkids0001' }
				},
				{ id: 'trhass0001', name: 'Home Assistant "lights off"', kind: 'http', action: { type: 'stop' } }
			],
			games: {
				enabled: true,
				matrixPropId: 'pmatrix001',
				port: 8088,
				gameSeconds: 60,
				cooldownMinutes: 5,
				levels: '',
				playWindow: 'duringShow',
				pauseShow: true,
				santaHat: true,
				arcadeMode: false,
				arcadeMinutes: 0,
				arcadeIdleSeconds: 600,
				publicUrl: 'play.chandlerlights.com',
				inviteEveryMinutes: 5,
				inviteStyle: 'alternate',
				inviteFlashes: 3,
				inviteColor: '#ff0000',
				scaleMode: 'fit',
				outputFps: 40,
				brightness: 100,
				volume: 80,
				crop: [8, 32, 256, 224]
			}
		}
	};
	return show;
}

/**
 * A brand-new show straight after the setup wizard: just the leader, nothing imported yet.
 * Used by `?mock=empty` so first-run guidance and every empty state can be seen and tested.
 */
export function buildEmptyShow(): Show {
	const demo = buildDemoShow();
	const leader = { ...demo.nodes[0], outputs: outputs('difftxlarge', 60) };
	return {
		version: 1,
		name: 'Maple Street Lights',
		nodes: [leader],
		receivers: [],
		props: [],
		propGroups: [],
		sequences: [],
		media: [],
		djClips: [],
		djVoices: [],
		pronunciations: [],
		effects: [],
		playlists: [],
		schedule: {
			enabled: false,
			location: demo.schedule.location,
			entries: []
		},
		settings: {
			...demo.settings,
			alerts: { rules: { ...demo.settings.alerts.rules } },
			requests: {
				enabled: false,
				maxQueue: 5,
				title: 'Request a song',
				message: 'Pick a song and it will play next. Merry Christmas!'
			},
			triggers: [],
			games: { ...demo.settings.games, enabled: false, matrixPropId: undefined, publicUrl: '' }
		}
	};
}

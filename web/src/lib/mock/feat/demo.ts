// Feature-wave demo data (WS0): the always-present settings sections with the daemon's
// defaults, plus example tags, analysis, a generated sequence, seasons, power supplies and a
// sensor node so every workstream's UI has something to show in demo mode.
import type { Show } from '$lib/api/types';

export function featureDemo(show: Show, { empty }: { empty: boolean }) {
	show.formatVersion = 1;
	Object.assign(show.settings, {
		https: { enabled: true },
		reports: {
			enabled: true,
			time: '07:00',
			email: true,
			push: true,
			onlyWhenProblems: false,
			keepDays: 90
		},
		power: { mode: 'warn', safety: 0.9, dim: [], maxBrightness: 100 },
		remote: { publicListener: false },
		updates: {
			channel: 'stable',
			auto: 'notify',
			window: { from: '10:00', to: '14:00', days: [] },
			avoidShowHours: 2
		},
		xlights: { fppConnect: false, addToPlaylists: true }
	});
	if (empty) return;

	const tags: Record<string, string[]> = {
		'Wizards in Winter': ['rock', 'upbeat'],
		'Carol of the Bells': ['classic'],
		"Mad Russian's Christmas": ['rock', 'upbeat'],
		'All I Want for Christmas Is You': ['kids', 'upbeat'],
		'Let It Go': ['kids'],
		'Jingle Bell Rock': ['kids', 'classic'],
		'Feliz Navidad': ['kids']
	};
	for (const s of show.sequences) if (tags[s.name]) s.tags = [...tags[s.name]];
	show.media.forEach((m, i) => {
		m.tags = tags[m.name] ? [...tags[m.name]] : undefined;
		m.analysis = {
			version: 1,
			bpm: [148, 120, 128, 132, 150, 137, 119, 148][i % 8],
			bpmConfidence: 0.82,
			beatCount: Math.round((m.durationMs / 60000) * 128),
			firstBeatMs: 240 + i * 17,
			energy: [0.8, 0.55, 0.85, 0.7, 0.75, 0.6, 0.5, 0.65][i % 8],
			sections: 6 + (i % 4)
		};
	});
	const letItGo = show.sequences.find((s) => s.name === 'Let It Go');
	if (letItGo)
		letItGo.generated = {
			kind: 'autoShow',
			mediaId: letItGo.mediaId ?? '',
			style: 'classic',
			propIds: [],
			seed: 42,
			analysisVersion: 1,
			propsHash: 'demo'
		};
	show.tagDefs = [
		{ name: 'kids', color: '#4ade80' },
		{ name: 'classic', color: '#fbbf24' },
		{ name: 'rock', color: '#f87171' },
		{ name: 'upbeat', color: '#60a5fa' }
	];
	show.nodes[0].serial = 'PPX1-TXL-00042';
	show.profiles = [
		{
			id: 'pfxmas0001',
			name: 'Christmas',
			icon: '🎄',
			color: '#22c55e',
			dateRange: { start: '11-15', end: '01-06' },
			priority: 1,
			schedule: structuredClone(show.schedule),
			tags: ['season:christmas']
		},
		{
			id: 'pfhallow01',
			name: 'Halloween',
			icon: '🎃',
			color: '#f97316',
			dateRange: { start: '10-01', end: '11-01' },
			priority: 0,
			schedule: { ...structuredClone(show.schedule), entries: [] },
			tags: ['season:halloween'],
			disabledPropIds: show.props.filter((p) => p.kind === 'candycane').map((p) => p.id)
		}
	];
	show.activeProfileId = 'pfxmas0001';
	show.profileAutoSwitch = true;
	show.powerSupplies = [
		{
			id: 'psfront001',
			name: 'Front yard PSU (350 W)',
			volts: 12,
			amps: 29,
			receiverIds: show.receivers.slice(0, 1).map((r) => r.id),
			directOutputs: []
		}
	];
	show.sensorNodes = [
		{
			id: 'snsidewk01',
			name: 'Sidewalk sensor',
			hw: 'esp32c3',
			location: 'By the mailbox',
			adopted: true,
			inputs: [
				{ id: 'pir1', name: 'Motion', pin: 4, kind: 'motion', activeLow: false, debounceMs: 30, holdMs: 0 },
				{
					id: 'btn1',
					name: 'Doorbell button',
					pin: 5,
					kind: 'button',
					activeLow: true,
					debounceMs: 30,
					holdMs: 0
				}
			]
		}
	];
}

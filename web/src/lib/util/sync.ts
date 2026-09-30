// How well a controller keeps time with the show leader (Controllers page badge).
import type { NodeStatus } from '$lib/api/types';

export type SyncLevel = 'excellent' | 'good' | 'fair' | 'poor' | 'unknown';

export interface SyncGrade {
	level: SyncLevel;
	/** Short badge text, e.g. "In sync ±0.3 ms". */
	label: string;
	/** Badge colour class. */
	tone: 'green' | 'accent' | 'red' | '';
	/** What to do about it (empty when all is well). */
	tips: string[];
}

/** The cluster protocol this UI's daemon speaks (matches `PROTOCOL_VERSION`). */
export const PROTOCOL_VERSION = 2;

/** "±0.3 ms", "±12 ms". */
export function fmtMs(ms: number): string {
	return `±${ms < 10 ? ms.toFixed(1) : Math.round(ms)} ms`;
}

/**
 * Grade a follower's timing (research report §9.1): excellent below 2 ms with Wi-Fi power
 * saving off, good below 5 ms, fair below one frame, poor beyond that, with power saving on,
 * over 10 % lost probes or another protocol version.
 */
export function syncGrade(
	n: Pick<NodeStatus, 'sync' | 'wifiPowerSave' | 'protocol' | 'online'>,
	frameMs = 25
): SyncGrade {
	const q = n.sync;
	const tips: string[] = [];
	if (n.protocol && n.protocol !== PROTOCOL_VERSION)
		return {
			level: 'poor',
			label: 'Update needed',
			tone: 'red',
			tips: ['This controller runs another PixelPlus version. Update every controller to the same version.']
		};
	if (!n.online || !q) return { level: 'unknown', label: 'Measuring…', tone: '', tips };
	const err = q.offsetErrorMs;
	if (n.wifiPowerSave)
		tips.push(
			'Wi-Fi power saving is on: update PixelPlus on it, or run “sudo iw dev wlan0 set power_save off”.'
		);
	if (q.lossPct > 10)
		tips.push(
			`${Math.round(q.lossPct)} % of timing packets are lost: move the Wi-Fi access point closer or use Ethernet.`
		);
	if (err >= 5)
		tips.push('The network delay varies a lot: use 5 GHz Wi-Fi, a less busy channel, or Ethernet.');
	if (q.refreshHz && q.refreshHz < 30)
		tips.push(
			`Long strings refresh only ${Math.round(q.refreshHz)} times a second (frame changes land within ${fmtMs(500 / q.refreshHz)}); split long strings across more ports for finer timing.`
		);
	let level: SyncLevel;
	if (n.wifiPowerSave || q.lossPct > 10 || err >= frameMs) level = 'poor';
	else if (err >= 5) level = 'fair';
	else if (err >= 2) level = 'good';
	else level = 'excellent';
	const tone = level === 'poor' ? 'red' : level === 'fair' ? 'accent' : 'green';
	return { level, label: `In sync ${fmtMs(err)}`, tone, tips };
}

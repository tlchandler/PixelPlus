import {
	AudioWaveform,
	CalendarClock,
	CalendarRange,
	Camera,
	ClipboardList,
	Cpu,
	Gamepad2,
	LayoutDashboard,
	ListMusic,
	Map as MapIcon,
	Mic,
	Music,
	Settings,
	Shapes,
	ShieldCheck,
	WandSparkles
} from '@lucide/svelte';
import type { Component } from 'svelte';

export interface NavItem {
	href: string;
	label: string;
	icon: Component<any>;
	key?: string;
	group: 'show' | 'build' | 'content' | 'extras' | 'system';
}

export const NAV: NavItem[] = [
	{ href: '/', label: 'Dashboard', icon: LayoutDashboard, key: 'd', group: 'show' },
	{ href: '/layout', label: 'Layout', icon: MapIcon, key: 'l', group: 'show' },
	{ href: '/props', label: 'Props', icon: Shapes, key: 'p', group: 'build' },
	{ href: '/controllers', label: 'Controllers', icon: Cpu, key: 'c', group: 'build' },
	{ href: '/sequences', label: 'Sequences & Audio', icon: Music, key: 'q', group: 'content' },
	{ href: '/playlists', label: 'Playlists', icon: ListMusic, key: 'y', group: 'content' },
	{ href: '/schedule', label: 'Schedule', icon: CalendarClock, key: 's', group: 'content' },
	{ href: '/dj', label: 'DJ Studio', icon: Mic, key: 'j', group: 'content' },
	{ href: '/effects', label: 'Effects', icon: WandSparkles, key: 'e', group: 'content' },
	{ href: '/games', label: 'Games', icon: Gamepad2, key: 'g', group: 'extras' },
	// Feature wave (ARCHITECTURE §12); pages owned by their workstreams.
	{ href: '/reports', label: 'Reports', icon: ClipboardList, group: 'show' },
	{ href: '/map', label: 'Map my yard', icon: Camera, group: 'build' },
	{ href: '/settings/seasons', label: 'Seasons', icon: CalendarRange, group: 'content' },
	{ href: '/calibrate', label: 'Sync to sound', icon: AudioWaveform, group: 'extras' },
	{ href: '/trust', label: 'Trust this phone', icon: ShieldCheck, group: 'extras' },
	{ href: '/settings', label: 'Settings', icon: Settings, key: ',', group: 'system' }
];

export const GROUPS: { id: NavItem['group']; label: string }[] = [
	{ id: 'show', label: 'Show' },
	{ id: 'build', label: 'Build' },
	{ id: 'content', label: 'Content' },
	{ id: 'extras', label: 'Extras' }
];

/** Phone tab bar: the pages people touch every night in the yard. Everything else is under More. */
export const TABS = ['/', '/props', '/playlists', '/schedule'];

export function isActive(href: string, path: string): boolean {
	if (href === '/') return path === '/';
	const within = (h: string) => path === h || path.startsWith(h + '/');
	// A more specific entry (e.g. /settings/seasons under /settings) wins.
	if (NAV.some((n) => n.href.startsWith(href + '/') && within(n.href))) return false;
	return within(href);
}

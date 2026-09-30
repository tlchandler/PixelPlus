import type { PropKind } from '$lib/api/types';
import {
	AppWindow,
	Asterisk,
	CandyCane,
	Circle,
	Grid3x3,
	Lightbulb,
	Minus,
	Rainbow,
	Shapes,
	Snowflake,
	Star,
	TreePine
} from '@lucide/svelte';
import type { Component } from 'svelte';

export const KIND_META: Record<PropKind, { label: string; icon: Component<any> }> = {
	arch: { label: 'Arch', icon: Rainbow },
	candycane: { label: 'Candy cane', icon: CandyCane },
	tree: { label: 'Tree', icon: TreePine },
	matrix: { label: 'Matrix', icon: Grid3x3 },
	line: { label: 'Line / roofline', icon: Minus },
	circle: { label: 'Circle / wreath', icon: Circle },
	star: { label: 'Star', icon: Star },
	spinner: { label: 'Spinner', icon: Asterisk },
	window: { label: 'Window frame', icon: AppWindow },
	icicles: { label: 'Icicles', icon: Snowflake },
	custom: { label: 'Custom', icon: Shapes },
	other: { label: 'Other', icon: Lightbulb }
};

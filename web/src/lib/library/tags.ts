// Tag helpers shared by the Sequences and Playlists pages (F18).
import type { Show, TagDef } from '$lib/api/types';

export const MAX_TAG_LEN = 32;

/** Same rules as the daemon (`smartlist::normalize_tags`): trimmed, lower case, no commas. */
export function normalizeTag(t: string): string {
	return t
		.replace(/,/g, ' ')
		.split(/\s+/)
		.filter(Boolean)
		.join(' ')
		.toLowerCase()
		.slice(0, MAX_TAG_LEN)
		.trim();
}

/** Split typed text ("kids, classic") into clean tags. */
export function parseTags(text: string): string[] {
	const out: string[] = [];
	for (const part of text.split(',')) {
		const t = normalizeTag(part);
		if (t && !out.includes(t)) out.push(t);
	}
	return out;
}

/** Every tag in the library with its use count, most used first. */
export function allTags(show: Show | null | undefined): { name: string; count: number }[] {
	const m = new Map<string, number>();
	for (const x of [...(show?.sequences ?? []), ...(show?.media ?? [])])
		for (const t of x.tags ?? []) m.set(t, (m.get(t) ?? 0) + 1);
	for (const d of show?.tagDefs ?? []) if (!m.has(d.name)) m.set(d.name, 0);
	return [...m.entries()]
		.map(([name, count]) => ({ name, count }))
		.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

/** Tags suggested before the owner has any. */
export const STARTER_TAGS = ['kids', 'classic', 'upbeat', 'slow', 'halloween', 'christmas'];

const PALETTE = ['#f5a524', '#3b82f6', '#22c55e', '#ef4444', '#a855f7', '#14b8a6', '#ec4899', '#eab308'];

/** A tag's colour: its TagDef colour, else a stable one from its name. */
export function tagColor(name: string, defs: TagDef[] | undefined): string {
	const d = defs?.find((x) => x.name === name);
	if (d?.color) return d.color;
	let h = 0;
	for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
	return PALETTE[h % PALETTE.length];
}

/** Energy words for the 0..1 mean normalized energy. */
export function energyWord(e: number): string {
	return e >= 0.62 ? 'High energy' : e >= 0.4 ? 'Lively' : 'Calm';
}

/** A four-step bar glyph for an energy value (▂▄▆█). */
export function energyGlyph(e: number): string {
	return ['▂', '▄', '▆', '█'][Math.max(0, Math.min(3, Math.floor(e * 4)))];
}

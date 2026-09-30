import type { Show } from '$lib/api/types';

/** "chandlerlights.com/request" → "https://chandlerlights.com/request". */
export function normalizeUrl(u: string): string {
	const t = u.trim();
	if (!t) return '';
	return /^https?:\/\//i.test(t) ? t : `https://${t}`;
}

/**
 * Where visitors open the song request page. With a public (internet) address it works from
 * anywhere; otherwise it's this controller's own address, which only works on the home Wi-Fi.
 */
export function requestLink(show: Show | null | undefined, origin: string): { url: string; isPublic: boolean } {
	const pub = show?.settings.requests.publicUrl?.trim();
	if (pub) return { url: normalizeUrl(pub), isPublic: true };
	return { url: `${origin.replace(/\/$/, '')}/request`, isPublic: false };
}

/** "88.3" → "88.3 FM"; "88.3 FM" stays. */
export function fmStation(f: string | null | undefined): string {
	const t = (f ?? '').trim();
	if (!t) return '';
	return /[a-z]/i.test(t) ? t : `${t} FM`;
}

/** URL without the scheme, for printing ("chandlerlights.com/request"). */
export const prettyUrl = (u: string) => u.replace(/^https?:\/\//, '').replace(/\/$/, '');

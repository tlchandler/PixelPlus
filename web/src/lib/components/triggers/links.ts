// Secret trigger links (ARCHITECTURE §12.18): the URLs and ready-to-paste snippets shown in
// Settings → Triggers. Pure functions (tested in links.test.ts).
import type { TriggerLinkAddress } from '$lib/api/types';

/** Placeholder used in snippets when the token isn't on screen any more. */
export const TOKEN_PLACEHOLDER = 'PASTE-YOUR-TOKEN-HERE';

export function hookPath(id: string): string {
	return `/api/v1/hooks/trigger/${encodeURIComponent(id)}`;
}

export function hookUrl(base: string, id: string): string {
	return base.replace(/\/+$/, '') + hookPath(id);
}

/** The all-in-one link (token in the query) for devices that only take a URL. */
export function withToken(url: string, token: string): string {
	return `${url}${url.includes('?') ? '&' : '?'}token=${encodeURIComponent(token)}`;
}

/** `Front door` → `front_door` (a Home Assistant service name part). */
export function slug(name: string): string {
	const s = name
		.normalize('NFKD')
		.replace(/[̀-ͯ]/g, '')
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '_')
		.replace(/^_+|_+$/g, '')
		.slice(0, 40);
	return s || 'trigger';
}

const yamlString = (s: string) => JSON.stringify(s);

/** A Home Assistant `rest_command` for configuration.yaml. */
export function homeAssistantYaml(
	name: string,
	url: string,
	token: string,
	opts: { https?: boolean } = {}
): string {
	const key = `pixelplus_${slug(name)}`;
	const lines = [
		'rest_command:',
		`  ${key}:`,
		`    url: ${yamlString(url)}`,
		'    method: POST',
		'    headers:',
		`      Authorization: ${yamlString(`Bearer ${token}`)}`
	];
	// The controller's own certificate authority isn't one Home Assistant knows.
	if (opts.https) lines.push('    verify_ssl: false');
	return lines.join('\n');
}

/** How to call it from an automation. */
export function homeAssistantAction(name: string): string {
	return `action: rest_command.pixelplus_${slug(name)}`;
}

export function curlCommand(url: string, token: string, opts: { https?: boolean } = {}): string {
	return `curl -X POST${opts.https ? ' -k' : ''} -H ${shellQuote(`Authorization: Bearer ${token}`)} ${shellQuote(url)}`;
}

function shellQuote(s: string): string {
	return /^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
}

/** The address to show first: the `.local` name, then an IP; internet only when allowed. */
export function usableAddresses(all: TriggerLinkAddress[], allowInternet: boolean): TriggerLinkAddress[] {
	const order = { name: 0, ip: 1, https: 2, internet: 3 } as const;
	return all
		.filter((a) => a.kind !== 'internet' || allowInternet)
		.slice()
		.sort((a, b) => order[a.kind] - order[b.kind]);
}

/** Fallback when the controller didn't list addresses (demo, old daemon): this page's origin. */
export function fallbackAddress(origin: string): TriggerLinkAddress {
	return { kind: 'name', label: origin.replace(/^https?:\/\//, ''), base: origin };
}

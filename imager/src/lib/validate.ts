// Client-side validation, mirroring imager/core/src/settings.rs (which re-validates
// before anything is written) so the form can show errors as you type.
import type { FieldError, ImagerSettings } from './types';

const utf8Len = (s: string) => new TextEncoder().encode(s).length;

export function normalizeHostname(h: string): string {
	return h.trim().replace(/\.local$/i, '').toLowerCase();
}

/** Suggest a hostname from free text: "Garage Tree!" -> "garage-tree". */
export function slugHostname(s: string): string {
	return s
		.toLowerCase()
		.normalize('NFKD')
		.replace(/[̀-ͯ]/g, '')
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.slice(0, 63)
		.replace(/-+$/g, '');
}

export function validHostname(h: string): boolean {
	return /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(h);
}

function validPsk(p: string): boolean {
	if (/^[0-9a-fA-F]{64}$/.test(p)) return true;
	const n = utf8Len(p);
	// eslint-disable-next-line no-control-regex
	return n >= 8 && n <= 63 && !/[\u0000-\u001f]/.test(p);
}

export function validate(s: ImagerSettings): FieldError[] {
	const e: FieldError[] = [];
	const add = (field: keyof ImagerSettings, message: string) => e.push({ field, message });
	const host = normalizeHostname(s.hostname);
	const country = s.wifiCountry.trim().toUpperCase();

	if (utf8Len(s.wifiSsid) > 32) add('wifiSsid', 'A Wi-Fi name can be at most 32 bytes.');
	if (s.wifiPassword) {
		if (!s.wifiSsid) add('wifiPassword', 'Enter the Wi-Fi name first.');
		else if (!validPsk(s.wifiPassword)) add('wifiPassword', 'Wi-Fi passwords are 8 to 63 characters.');
	}
	if (country && !/^[A-Z]{2}$/.test(country)) add('wifiCountry', 'Choose a country.');
	if (s.wifiSsid && !country) add('wifiCountry', 'Wi-Fi needs the country it is used in.');
	if (host && !validHostname(host)) add('hostname', 'Use letters, numbers and dashes (not at the start or end).');
	if (s.uiPassword && [...s.uiPassword].length < 6) add('uiPassword', 'Use at least 6 characters.');
	if (s.sshPassword && (s.sshPassword.length < 8 || s.sshPassword.includes(':')))
		add('sshPassword', "Use at least 8 characters (no ':').");
	if (s.ssh && !s.sshPassword && !s.sshKey.trim()) add('sshPassword', 'Set a password or a key, or turn SSH off.');
	const key = s.sshKey.trim();
	if (key && !/^(ssh-ed25519 |ssh-rsa |ecdsa-sha2-|sk-ssh-ed25519@openssh\.com |sk-ecdsa-sha2-)/.test(key))
		add('sshKey', 'Paste a public key (ssh-ed25519 AAAA...).');
	return e;
}

export function errorFor(errors: FieldError[], field: keyof ImagerSettings): string | undefined {
	return errors.find((x) => x.field === field)?.message;
}

/** "en-US" -> "US"; used to pre-select the Wi-Fi country. */
export function countryFromLocale(locale: string | undefined): string {
	const m = /[-_]([A-Za-z]{2})(?:$|[-_.@])/.exec(locale ?? '');
	return m ? m[1].toUpperCase() : '';
}

export function formatBytes(n: number | null | undefined): string {
	if (!n) return '–';
	if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
	if (n >= 1e6) return `${(n / 1e6).toFixed(0)} MB`;
	return `${Math.round(n / 1e3)} kB`;
}

export function formatEta(seconds: number): string {
	if (!isFinite(seconds) || seconds <= 0) return '';
	if (seconds < 60) return `${Math.ceil(seconds)} s left`;
	return `${Math.ceil(seconds / 60)} min left`;
}

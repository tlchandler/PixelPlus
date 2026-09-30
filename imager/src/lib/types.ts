// Shapes shared with the Rust side (serde camelCase) - see imager/core and src-tauri.

export type Role = 'leader' | 'follower';

export interface ImagerSettings {
	wifiSsid: string;
	wifiPassword: string;
	wifiCountry: string;
	wifiHidden: boolean;
	hostname: string;
	role: Role | null;
	timezone: string;
	uiPassword: string;
	ssh: boolean;
	sshPassword: string;
	sshKey: string;
}

export interface FieldError {
	field: keyof ImagerSettings;
	message: string;
}

export interface OsImage {
	name: string;
	description: string;
	url: string;
	releaseDate: string;
	version: string;
	prerelease: boolean;
	downloadSize: number | null;
	downloadSha256: string | null;
	extractSize: number | null;
	extractSha256: string | null;
	recommended: boolean;
}

/** What will be written: a release (downloaded first) or a local file. */
export type ImageChoice =
	| { kind: 'release'; image: OsImage }
	| { kind: 'file'; path: string; name: string; size: number };

export interface Drive {
	device: string;
	name: string;
	size: number;
	bus: string;
	mountpoints: string[];
	tooSmall: boolean;
}

export type Phase = 'download' | 'prepare' | 'write' | 'verify' | 'customize' | 'done' | 'error';

export interface Progress {
	phase: Phase;
	bytes: number;
	total: number | null;
	message: string | null;
}

export interface Defaults {
	timezone: string;
	country: string;
	timezones: string[];
	platform: 'linux' | 'macos' | 'windows' | 'unknown';
}

export const emptySettings = (d?: Partial<Defaults>): ImagerSettings => ({
	wifiSsid: '',
	wifiPassword: '',
	wifiCountry: d?.country ?? '',
	wifiHidden: false,
	hostname: 'pixelplus',
	role: 'leader',
	timezone: d?.timezone ?? '',
	uiPassword: '',
	ssh: false,
	sshPassword: '',
	sshKey: ''
});

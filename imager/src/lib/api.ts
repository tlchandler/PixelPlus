// Bridge to the Rust side (src-tauri/src/commands.rs). When the UI runs in a plain
// browser (`pnpm dev` without Tauri) a mock backend is used so the flow can be designed
// and tested without hardware.
import type { Defaults, Drive, FieldError, ImagerSettings, OsImage, Progress } from './types';
import { countryFromLocale } from './validate';

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
	const { invoke } = await import('@tauri-apps/api/core');
	return invoke<T>(cmd, args);
}

async function listen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
	const { listen } = await import('@tauri-apps/api/event');
	return listen<T>(event, (e) => cb(e.payload));
}

export interface Backend {
	defaults(): Promise<Defaults>;
	releases(): Promise<OsImage[]>;
	pickImageFile(): Promise<{ path: string; name: string; size: number } | null>;
	listDrives(): Promise<Drive[]>;
	validate(settings: ImagerSettings): Promise<FieldError[]>;
	/** Download (if needed) + write + verify + customise. Progress events until done/error. */
	write(
		req: { image: OsImage | null; localPath: string | null; device: string; settings: ImagerSettings },
		onProgress: (p: Progress) => void
	): Promise<void>;
	cancel(): Promise<void>;
}

const tauriBackend: Backend = {
	defaults: () => invoke<Defaults>('defaults'),
	releases: () => invoke<OsImage[]>('releases'),
	async pickImageFile() {
		const { open } = await import('@tauri-apps/plugin-dialog');
		const path = await open({
			multiple: false,
			directory: false,
			title: 'Choose a PixelPlus image',
			filters: [{ name: 'Raspberry Pi image', extensions: ['img', 'xz'] }]
		});
		if (!path || Array.isArray(path)) return null;
		return invoke<{ path: string; name: string; size: number }>('inspect_image', { path });
	},
	listDrives: () => invoke<Drive[]>('list_drives'),
	validate: (settings) => invoke<FieldError[]>('validate_settings', { settings }),
	async write(req, onProgress) {
		const un = await listen<Progress>('write-progress', onProgress);
		try {
			await invoke('write_card', { req });
		} finally {
			un();
		}
	},
	cancel: () => invoke('cancel_write')
};

// ---------------------------------------------------------------------------
// browser mock
// ---------------------------------------------------------------------------
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let mockCancelled = false;

const mockBackend: Backend = {
	async defaults() {
		const tz = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
		const zones = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone') ?? [tz];
		return { timezone: tz, country: countryFromLocale(navigator.language) || 'US', timezones: zones, platform: 'unknown' };
	},
	async releases() {
		await sleep(400);
		return [
			{
				name: 'PixelPlus 0.1.0 (Trixie, 64-bit)',
				description: 'Recommended for all boards.',
				url: 'https://example.invalid/pixelplus-0.1.0-trixie-arm64.img.xz',
				releaseDate: '2026-10-01',
				version: '0.1.0',
				prerelease: false,
				downloadSize: 912_000_000,
				downloadSha256: null,
				extractSize: 3_200_000_000,
				extractSha256: null,
				recommended: true
			},
			{
				name: 'PixelPlus 0.1.0 (Bookworm, 64-bit)',
				description: 'For compatibility with older setups.',
				url: 'https://example.invalid/pixelplus-0.1.0-bookworm-arm64.img.xz',
				releaseDate: '2026-10-01',
				version: '0.1.0',
				prerelease: false,
				downloadSize: 870_000_000,
				downloadSha256: null,
				extractSize: 3_000_000_000,
				extractSha256: null,
				recommended: false
			}
		];
	},
	async pickImageFile() {
		return { path: '/home/me/Downloads/pixelplus-dev.img.xz', name: 'pixelplus-dev.img.xz', size: 880_000_000 };
	},
	async listDrives() {
		await sleep(200);
		return [
			{ device: '/dev/sdb', name: 'Generic SD/MMC (31.9 GB)', size: 31_914_983_424, bus: 'usb', mountpoints: ['/media/me/bootfs'], tooSmall: false },
			{ device: '/dev/sdc', name: 'SanDisk Ultra (2.0 GB)', size: 2_000_000_000, bus: 'usb', mountpoints: [], tooSmall: true }
		];
	},
	async validate() {
		return [];
	},
	async write(_req, onProgress) {
		mockCancelled = false;
		const total = 3_200_000_000;
		const steps: [Progress['phase'], number][] = [['download', 900_000_000], ['write', total], ['verify', total]];
		onProgress({ phase: 'prepare', bytes: 0, total: null, message: 'Preparing the card' });
		for (const [phase, t] of steps) {
			for (let i = 0; i <= 40; i++) {
				if (mockCancelled) throw new Error('cancelled');
				onProgress({ phase, bytes: (t * i) / 40, total: t, message: null });
				await sleep(60);
			}
		}
		onProgress({ phase: 'customize', bytes: 0, total: null, message: 'Saving your settings (pixelplus.txt)' });
		await sleep(400);
		onProgress({ phase: 'done', bytes: 0, total: null, message: 'Done' });
	},
	async cancel() {
		mockCancelled = true;
	}
};

export const api: Backend = inTauri ? tauriBackend : mockBackend;
export const isMock = !inTauri;

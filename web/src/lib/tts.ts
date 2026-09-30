// Chooses where DJ speech is rendered: on the device (Kokoro sidecar, Pi 4/5 / Docker) or
// in this browser (lazy-loaded tts-browser module, for Pi Zero 2 W / Pi 3 leaders).
import { api } from '$lib/api/client';
import type { DjLine, Show, TtsStatus } from '$lib/api/types';

let status: Promise<TtsStatus | null> | null = null;

export function ttsStatus(): Promise<TtsStatus | null> {
	status ??= api.ttsStatus().catch(() => null);
	return status;
}

export async function renderWhere(show: Show): Promise<'device' | 'browser'> {
	const mode = show.settings.tts.mode;
	if (mode === 'device' || mode === 'browser') return mode;
	const st = await ttsStatus();
	return st?.available && st.mode === 'device' ? 'device' : 'browser';
}

/** Render lines to audio (for auditions and previews). */
export async function renderSpeech(show: Show, lines: DjLine[], speed: number, onProgress?: (p: number) => void): Promise<Blob> {
	const where = await renderWhere(show);
	if (where === 'device') {
		onProgress?.(0.2);
		const blob = await api.ttsRender(lines, speed);
		onProgress?.(1);
		return blob;
	}
	const tts = await import('$lib/tts-browser');
	if (!(await tts.isBrowserTtsSupported()))
		throw new Error('This browser can’t render voices. Try a recent Chrome, Edge or Safari on a computer.');
	return tts.renderDialog(
		lines.map((l) => ({ voice: l.voice, text: l.text, pauseMs: l.pauseMs, energy: l.energy })),
		{
			speed,
			onProgress,
			voices: show.djVoices,
			pronunciations: show.pronunciations,
			loudnessLufs: show.settings.audio.targetLufs
		}
	);
}

let current: HTMLAudioElement | null = null;
export function playBlob(blob: Blob, onend?: () => void): () => void {
	current?.pause();
	const url = URL.createObjectURL(blob);
	const a = new Audio(url);
	current = a;
	const done = () => {
		URL.revokeObjectURL(url);
		onend?.();
	};
	a.onended = done;
	a.play().catch(done);
	return () => {
		a.pause();
		done();
	};
}

/** Fill placeholders with sample values for previews. */
export function sampleText(text: string, show: Show): string {
	const now = new Date();
	const xmas = new Date(now.getFullYear(), 11, 25);
	if (xmas < now) xmas.setFullYear(now.getFullYear() + 1);
	const vals: Record<string, string> = {
		time: now.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }),
		date: now.toLocaleDateString(undefined, { month: 'long', day: 'numeric' }),
		day: now.toLocaleDateString(undefined, { weekday: 'long' }),
		daysUntilChristmas: String(Math.ceil((xmas.getTime() - now.getTime()) / 86400000)),
		nextSong: show.sequences[1]?.name ?? 'Carol of the Bells',
		prevSong: show.sequences[0]?.name ?? 'Wizards in Winter',
		showName: show.name,
		temperature: '34 degrees',
		sunset: '4:32 PM',
		requestName: 'Emma'
	};
	return text.replace(/\{(\w+)\}/g, (m, k) => vals[k] ?? m);
}

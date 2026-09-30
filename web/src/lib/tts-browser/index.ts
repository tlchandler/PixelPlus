// PLACEHOLDER STUB — owned by the TTS engineer, who will replace this file with the
// real in-browser Kokoro renderer. It exists only so the web UI builds before that lands.

export interface DjVoiceLike {
	id: string;
	name?: string;
	blend: Record<string, number>;
	speed?: number;
	lang?: string;
	defaultEnergy?: number;
	energy?: Record<string, number>;
}

export async function isBrowserTtsSupported(): Promise<boolean> {
	return false;
}

export async function listBrowserVoices(): Promise<
	{ id: string; name: string; language: string; gender: 'male' | 'female' }[]
> {
	return [];
}

export async function renderDialog(
	_lines: { voice: DjVoiceLike; text: string; pauseMs: number; energy?: number }[],
	_opts: { speed: number; onProgress?: (p: number) => void }
): Promise<Blob> {
	throw new Error('In-browser voice rendering is not available in this build yet.');
}

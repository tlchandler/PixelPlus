/** This phone's model (for the per-phone timing bias) and the bias store (localStorage). */

type UAData = { getHighEntropyValues?: (hints: string[]) => Promise<{ model?: string; platform?: string }> };

/** "Pixel 8", "SM-S911B", "iPhone"… or "This phone". */
export async function deviceModel(): Promise<string> {
	const uad = (navigator as Navigator & { userAgentData?: UAData }).userAgentData;
	try {
		const v = await uad?.getHighEntropyValues?.(['model', 'platform']);
		if (v?.model) return v.model.slice(0, 60);
	} catch {
		/* not allowed */
	}
	return modelFromUA(navigator.userAgent);
}

/** Best-effort model from a user-agent string. */
export function modelFromUA(ua: string): string {
	const android = /Android [\d.]+; (?:[a-z]{2}[-_][a-z]{2}; )?([^;)]+?)(?: Build\/[^;)]*)?[;)]/i.exec(ua);
	if (android && android[1] && android[1] !== 'K') return android[1].trim().slice(0, 60);
	if (/iPhone/.test(ua)) return 'iPhone';
	if (/iPad/.test(ua)) return 'iPad';
	if (/Android/.test(ua)) return 'Android phone';
	return 'This device';
}

export interface StoredBias {
	biasMs: number;
	spreadMs: number;
	pairs: number;
	/** ISO time measured. */
	at: string;
}

const key = (model: string) => `pp.avBias.${model}`;

export function loadBias(model: string): StoredBias | null {
	try {
		const raw = localStorage.getItem(key(model));
		if (!raw) return null;
		const b = JSON.parse(raw) as StoredBias;
		return Number.isFinite(b.biasMs) ? b : null;
	} catch {
		return null;
	}
}

export function saveBias(model: string, b: StoredBias): void {
	try {
		localStorage.setItem(key(model), JSON.stringify(b));
	} catch {
		/* private mode */
	}
}

export function clearBias(model: string): void {
	try {
		localStorage.removeItem(key(model));
	} catch {
		/* ignore */
	}
}

/** Keep the screen on while measuring (released automatically when the page hides). */
export async function keepAwake(): Promise<() => void> {
	try {
		const lock = await (
			navigator as Navigator & { wakeLock?: { request(t: 'screen'): Promise<{ release(): Promise<void> }> } }
		).wakeLock?.request('screen');
		return () => void lock?.release().catch(() => {});
	} catch {
		return () => {};
	}
}

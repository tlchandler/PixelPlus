// The fpp-voices DJ script format, same semantics as tts/pixelplus_tts/script.py:
//
//   # comment (hash + space); blank lines ignored
//   nick: Good evening, and welcome to the show!
//   holly!: Now sit back, relax, and *enjoy the show!*     (! hype = 1, !! extra hype = 1.5)
//   [pause 1.0]                                             (seconds; also 1.5s / 500ms)
//
// pauseMs is the silence after a line; [pause] adds to the previous line's pauseMs (a pause
// before any line becomes a text-less line). *asterisks* stay in the text (they pick the punchline).

export interface ScriptLine {
	voice: string;
	text: string;
	pauseMs: number;
	energy?: number;
}

export class ScriptError extends Error {
	constructor(
		public line: number,
		message: string
	) {
		super(`line ${line}: ${message}`);
		this.name = 'ScriptError';
	}
}

const PAUSE_RE = /^\[pause\s+(\d+(?:\.\d+)?|\.\d+)\s*(ms|s)?\s*\]$/i;
const VOICE_RE = /^([A-Za-z0-9_][A-Za-z0-9_ .'-]*?)\s*(!*)$/;

/** `resolve(name)` maps names/aliases to a voice id and throws for unknown voices. */
export function parseScript(text: string, resolve?: (name: string) => string): ScriptLine[] {
	const lines: ScriptLine[] = [];
	const rows = text.split(/\r\n|\r|\n/);
	for (let n = 1; n <= rows.length; n++) {
		const line = rows[n - 1].trim();
		if (!line || line === '#' || line.startsWith('# ')) continue;
		const pm = PAUSE_RE.exec(line);
		if (pm) {
			const ms = Math.round(Number(pm[1]) * ((pm[2] ?? 's').toLowerCase() === 'ms' ? 1 : 1000));
			if (lines.length) lines[lines.length - 1].pauseMs += ms;
			else lines.push({ voice: '', text: '', pauseMs: ms });
			continue;
		}
		const colon = line.indexOf(':');
		if (colon < 0) throw new ScriptError(n, "expected 'voice: text' or '[pause N]'");
		const who = line.slice(0, colon).trim();
		const say = line.slice(colon + 1).trim();
		const vm = VOICE_RE.exec(who);
		if (!vm) throw new ScriptError(n, `bad voice name '${who}'`);
		const name = vm[1].trim();
		const bangs = vm[2].length;
		if (!say) throw new ScriptError(n, 'nothing to say');
		let voice: string;
		try {
			voice = resolve ? resolve(name) : name.toLowerCase();
		} catch (e) {
			throw new ScriptError(n, e instanceof Error ? e.message : String(e));
		}
		const item: ScriptLine = { voice, text: say, pauseMs: 0 };
		if (bangs) item.energy = bangs === 1 ? 1.0 : 1.5;
		lines.push(item);
	}
	return lines;
}

/** Lines -> script text (inverse of parseScript for the ! / !! energies). */
export function formatScript(lines: ScriptLine[], names: Record<string, string> = {}): string {
	const out: string[] = [];
	for (const ln of lines) {
		if (ln.text) {
			const e = ln.energy;
			const bang = e === undefined || e < 1 ? '' : e >= 1.5 ? '!!' : '!';
			out.push(`${names[ln.voice] ?? ln.voice}${bang}: ${ln.text}`);
		}
		if (ln.pauseMs) out.push(`[pause ${Number((ln.pauseMs / 1000).toFixed(3))}]`);
	}
	return out.join('\n') + '\n';
}

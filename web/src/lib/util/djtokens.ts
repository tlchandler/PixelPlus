/** Friendly names for the live-info placeholders a DJ line can contain ({daysUntilChristmas}…). */
export const PLACEHOLDER_LABELS: Record<string, string> = {
	time: 'Current time',
	date: 'Today’s date',
	day: 'Day of the week',
	daysUntilChristmas: 'Days until Christmas',
	nextSong: 'Next song',
	prevSong: 'Song that just played',
	showName: 'Show name',
	temperature: 'Temperature',
	sunset: 'Sunset time',
	requestName: 'Who requested the song'
};

export const placeholderLabel = (key: string) => PLACEHOLDER_LABELS[key] ?? key;

export type TextPart = { text: string } | { token: string; label: string };

/** "It's {time}!" → [{text:"It's "}, {token:"time", label:"Current time"}, {text:"!"}] */
export function tokenize(text: string): TextPart[] {
	const out: TextPart[] = [];
	const re = /\{(\w+)\}/g;
	let last = 0;
	for (let m = re.exec(text); m; m = re.exec(text)) {
		if (m.index > last) out.push({ text: text.slice(last, m.index) });
		out.push({ token: m[1], label: placeholderLabel(m[1]) });
		last = m.index + m[0].length;
	}
	if (last < text.length) out.push({ text: text.slice(last) });
	return out;
}

/** Pause after a line, in words. */
export const PAUSES = [
	{ ms: 0, label: 'No pause' },
	{ ms: 300, label: 'Short pause' },
	{ ms: 700, label: 'Medium pause' },
	{ ms: 1200, label: 'Long pause' }
];

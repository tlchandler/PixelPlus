const ALPHABET = 'abcdefghijklmnopqrstuvwxyz0123456789';

/** 10-char `[a-z0-9]` id, same shape as the daemon's `new_id()`. */
export function newId(): string {
	let s = '';
	const buf = new Uint8Array(10);
	crypto.getRandomValues(buf);
	for (const b of buf) s += ALPHABET[b % ALPHABET.length];
	return s;
}

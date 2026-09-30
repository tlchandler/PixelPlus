// WebSocket transport (swappable for the mock backend).
export interface SocketLike {
	binaryType: BinaryType;
	readyState: number;
	onopen: ((ev: any) => void) | null;
	onclose: ((ev: any) => void) | null;
	onerror: ((ev: any) => void) | null;
	onmessage: ((ev: { data: any }) => void) | null;
	send(data: string): void;
	close(): void;
}

let factory: () => SocketLike = () => {
	const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return new WebSocket(`${proto}//${location.host}/api/v1/ws`) as unknown as SocketLike;
};

export function setSocketFactory(f: () => SocketLike) {
	factory = f;
}
export function openSocket(): SocketLike {
	return factory();
}

/** Parse a binary preview frame: u8 0x50 | u32 frameNo | RGB bytes for every prop in show order. */
export function parsePreviewFrame(buf: ArrayBuffer): { frameNo: number; rgb: Uint8Array } | null {
	if (buf.byteLength < 5) return null;
	const v = new DataView(buf);
	if (v.getUint8(0) !== 0x50) return null;
	return { frameNo: v.getUint32(1, true), rgb: new Uint8Array(buf, 5) };
}

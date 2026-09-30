// Simulated peers for the e2e scenario: an ESP32 sensor node (firmware/esp32-sensor,
// protocol v1, ARCHITECTURE §7.5 / §12.16) and a stand-in games controller (HTTP +
// WebSocket echo) behind the public listener's /play proxy.
import crypto from 'node:crypto';
import dgram from 'node:dgram';
import http from 'node:http';

const hmac = (key, data) => crypto.createHmac('sha256', key).update(data).digest();
/** DER prefix of an X25519 SubjectPublicKeyInfo; the raw 32-byte key follows. */
const X25519_SPKI = Buffer.from('302a300506032b656e032100', 'hex');

/**
 * A sensor node: announces itself on the leader's sensor port, answers the adoption key
 * exchange over HTTP, then sends MACed heartbeats and input events and reads the acks.
 */
export class SimSensor {
	constructor({ id, sensorPort, name = 'E2E yard sensor', inputs = ['pir1'] }) {
		this.id = id;
		this.name = name;
		this.inputs = inputs;
		this.sensorPort = sensorPort;
		this.boot = crypto.randomBytes(4).toString('hex');
		this.seq = 1;
		this.key = null;
		this.leaderId = null;
		this.leaderBoot = '';
		this.acks = [];
		this.startedAt = Date.now();
	}

	async start() {
		this.http = http.createServer((req, res) => this.#onHttp(req, res));
		await new Promise((r) => this.http.listen(0, '127.0.0.1', r));
		this.httpPort = this.http.address().port;
		this.udp = dgram.createSocket('udp4');
		this.udp.on('message', (m) => this.#onDatagram(m));
		await new Promise((r) => this.udp.bind(0, '127.0.0.1', r));
		this.beacon();
		this.timer = setInterval(() => this.beacon(), 1000);
		return this;
	}

	stop() {
		clearInterval(this.timer);
		this.udp?.close();
		this.http?.close();
	}

	#send(buf) {
		return new Promise((r) => this.udp.send(buf, this.sensorPort, '127.0.0.1', () => r()));
	}

	beacon() {
		const b = {
			t: 'sbeacon',
			id: this.id,
			name: this.name,
			hw: 'esp32c3',
			ver: '0.1.0-sim',
			http: this.httpPort,
			adoptedBy: this.leaderId,
			inputs: this.inputs,
			proto: 1
		};
		return this.#send(Buffer.from(JSON.stringify(b)));
	}

	/** Seal a message like the firmware: `,"bt":…,"sq":…` then `,"mac":"<hmac>"`. */
	seal(obj, key = this.key) {
		const json = JSON.stringify(obj);
		const body = `${json.slice(0, -1)},"bt":${JSON.stringify(this.boot)},"sq":${this.seq++}}`;
		const mac = hmac(Buffer.from(key), body).toString('hex');
		return Buffer.from(`${body.slice(0, -1)},"mac":"${mac}"}`);
	}

	#onHttp(req, res) {
		let data = '';
		req.on('data', (c) => (data += c));
		req.on('end', () => {
			if (req.method === 'POST' && req.url === '/adopt') {
				const body = JSON.parse(data);
				const { privateKey, publicKey } = crypto.generateKeyPairSync('x25519');
				const mine = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32).toString('hex');
				const leaderKey = crypto.createPublicKey({
					key: Buffer.concat([X25519_SPKI, Buffer.from(body.dh, 'hex')]),
					format: 'der',
					type: 'spki'
				});
				const shared = crypto.diffieHellman({ privateKey, publicKey: leaderKey });
				const info = `pixelplus-sensor-key-v1\n${body.leaderId}\n${this.id}\n${body.dh}\n${mine}`;
				this.key = hmac(shared, info).toString('hex');
				this.leaderId = body.leaderId;
				const proof = hmac(Buffer.from(this.key), `pixelplus-sensor-adopted-v1\n${body.leaderId}\n${this.id}`);
				res.writeHead(200, { 'content-type': 'application/json' });
				res.end(
					JSON.stringify({
						id: this.id,
						dh: mine,
						proof: proof.toString('hex'),
						hw: 'esp32c3',
						ver: '0.1.0-sim',
						inputs: this.inputs.map((id, i) => ({ id, pin: 4 + i, kind: 'motion', activeLow: false }))
					})
				);
				return;
			}
			res.writeHead(404);
			res.end();
		});
	}

	#onDatagram(buf) {
		let m;
		try {
			m = JSON.parse(buf.toString());
		} catch {
			return;
		}
		if (m.t !== 'sack' || !this.key) return;
		const text = buf.toString();
		const cut = text.lastIndexOf(',"mac":"');
		const expect = hmac(Buffer.from(this.key), text.slice(0, cut) + '}').toString('hex');
		if (m.mac !== expect || m.sb !== this.boot) return;
		this.leaderBoot = m.lb;
		this.acks.push(m);
	}

	async #acked(sq, timeout = 2000) {
		const end = Date.now() + timeout;
		while (Date.now() < end) {
			const a = this.acks.find((x) => x.ack === sq);
			if (a) return a;
			await new Promise((r) => setTimeout(r, 20));
		}
		return null;
	}

	/** A heartbeat; resolves with the leader's ack (null when none came). */
	async status(extra = {}) {
		const sq = this.seq;
		await this.#send(
			this.seal({
				t: 'sstatus',
				id: this.id,
				rssi: -58,
				uptime: Math.round((Date.now() - this.startedAt) / 1000),
				ver: '0.1.0-sim',
				cfg: '',
				inputs: Object.fromEntries(this.inputs.map((i) => [i, 0])),
				amps: {},
				volts: {},
				...extra
			})
		);
		return this.#acked(sq);
	}

	/** An input change; `key` forges the MAC with another key. Resolves with the ack or null. */
	async event(input, state, { key } = {}) {
		const sq = this.seq;
		await this.#send(
			this.seal(
				{ t: 'sevent', id: this.id, input, state, ms: Date.now() - this.startedAt, lb: this.leaderBoot },
				key ?? this.key
			)
		);
		return this.#acked(sq, key ? 600 : 2000);
	}
}

/** A stand-in games controller: records requests, serves a page, echoes WebSocket text. */
export async function fakeGames() {
	const seen = [];
	const srv = http.createServer((req, res) => {
		seen.push({ url: req.url, xff: req.headers['x-forwarded-for'] ?? '' });
		res.writeHead(200, { 'content-type': 'text/html' });
		res.end('<h1>fake games controller</h1>');
	});
	srv.on('upgrade', (req, sock) => {
		seen.push({ url: req.url, xff: req.headers['x-forwarded-for'] ?? '', upgrade: true });
		const accept = crypto
			.createHash('sha1')
			.update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
			.digest('base64');
		sock.write(
			`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`
		);
		let buf = Buffer.alloc(0);
		sock.on('error', () => {});
		sock.on('data', (d) => {
			buf = Buffer.concat([buf, d]);
			while (buf.length >= 2) {
				const op = buf[0] & 0x0f;
				const masked = buf[1] & 0x80;
				let len = buf[1] & 0x7f;
				let off = 2;
				if (len === 126) {
					if (buf.length < 4) return;
					len = buf.readUInt16BE(2);
					off = 4;
				}
				const need = off + (masked ? 4 : 0) + len;
				if (buf.length < need) return;
				let payload = buf.subarray(off + (masked ? 4 : 0), need);
				if (masked) {
					const mask = buf.subarray(off, off + 4);
					payload = Buffer.from(payload.map((b, i) => b ^ mask[i % 4]));
				}
				buf = buf.subarray(need);
				if (op === 8) {
					sock.end();
					return;
				}
				if (op === 1) {
					const out = Buffer.from('echo:' + payload.toString());
					sock.write(Buffer.concat([Buffer.from([0x81, out.length]), out]));
				}
			}
		});
	});
	await new Promise((r) => srv.listen(0, '127.0.0.1', r));
	return {
		port: srv.address().port,
		seen,
		close: () => {
			srv.closeAllConnections?.();
			srv.close();
		}
	};
}

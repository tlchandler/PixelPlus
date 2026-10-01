#!/usr/bin/env node
// End-to-end scenario against a REAL three-node cluster (leader + two followers on
// this machine, started with scripts/dev-cluster.sh): setup wizard, discovery and
// adoption, xLights import, .fseq + audio upload, follower slices (byte-checked),
// playlist + schedule, frame-accurate and millisecond sync across nodes (via GET /debug/output),
// sync quality reports, "Sync lights to sound" calibration,
// tools (tests, fault finder, blackout, brightness, looks, overlays, requests,
// snapshots, health, power, sensors, games/TTS without sidecars), password,
// follower restart mid-show, leader crash, live prop changes, remove + re-adopt;
// engine features: countdown, power limiter + dimming (a follower's limiting seen by the leader),
// surprises, calibration v2, identify; fleet (updates, remote access, transfer file);
// the public-only listener (public pages only, /play proxied over HTTP and WebSocket,
// per-visitor request caps, Settings → Features turning Games and Song requests off/on); and a simulated ESP32 sensor node (adoption key exchange,
// MACed heartbeats and events firing a sensor surprise, forged packets ignored); and secret trigger
// links (a password set, curl fires a surprise with the token alone, tunnel rules, rotate / revoke).
//
//   cargo build -p pixelplus-daemon && (cd web && pnpm build)
//   node scripts/e2e/run.mjs            # fresh cluster in a temp dir, stopped afterwards
//   node scripts/e2e/run.mjs --keep     # leave the cluster running (e.g. for the Playwright
//                                       # suite: cd web && pnpm test:real)
//   node scripts/e2e/run.mjs --only=sync,tools   # stop after the named phases
//
// Environment: PP_BIN, PP_WEB_DIR, PP_HTTP_BASE, PP_CLUSTER_BASE, PP_PUBLIC_BASE, PP_CLUSTER_DIR
// (see scripts/dev-cluster.sh). Needs Node >= 22.15 (zstd, WebSocket).
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { channelCount, fillFrame, MARKER_B, nodeFrame, readPpseq, writeFseq, writeWav } from './media.mjs';
import { fakeGames, SimSensor } from './sims.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const args = new Set(process.argv.slice(2));
const KEEP = args.has('--keep');
const ONLY = [...args]
	.find((a) => a.startsWith('--only='))
	?.slice(7)
	.split(',');
const OWN_DIR = !process.env.PP_CLUSTER_DIR;
const DIR = process.env.PP_CLUSTER_DIR ?? fs.mkdtempSync(path.join(os.tmpdir(), 'pixelplus-e2e-'));
process.env.PP_CLUSTER_DIR = DIR;
const HTTP_BASE = Number(process.env.PP_HTTP_BASE ?? 18080);
const URL_OF = { leader: HTTP_BASE, f1: HTTP_BASE + 1, f2: HTTP_BASE + 2 };
const FRAME_MS = 50;
const SONG_FRAMES = 400; // 20 s

// ---------------------------------------------------------------------------
// Tiny test harness
// ---------------------------------------------------------------------------

const results = [];
let current = '';
function log(...a) {
	console.log(`  ${current ? `[${current}] ` : ''}${a.join(' ')}`);
}
class Fail extends Error {}
function check(cond, msg) {
	if (!cond) throw new Fail(msg);
}
function eq(a, b, msg) {
	if (JSON.stringify(a) !== JSON.stringify(b))
		throw new Fail(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, fn, { timeout = 15000, every = 250 } = {}) {
	const t0 = Date.now();
	let last;
	while (Date.now() - t0 < timeout) {
		try {
			last = await fn();
			if (last) return last;
		} catch (e) {
			last = e;
		}
		await sleep(every);
	}
	throw new Fail(
		`timed out after ${timeout} ms waiting for ${what}${last instanceof Error ? ` (${last.message})` : ''}`
	);
}
async function step(name, fn) {
	current = name;
	const t0 = Date.now();
	try {
		await fn();
		results.push({ name, ok: true, ms: Date.now() - t0 });
		console.log(`✓ ${name} (${Date.now() - t0} ms)`);
	} catch (e) {
		results.push({ name, ok: false, ms: Date.now() - t0, error: e.message });
		console.log(`✗ ${name}\n    ${e instanceof Fail ? e.message : e.stack}`);
	}
	current = '';
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

function client(node) {
	const base = `http://127.0.0.1:${URL_OF[node]}/api/v1`;
	let cookie = '';
	async function call(method, p, body, { expect = 200, raw = false, headers: extra = {} } = {}) {
		// Like the web UI: mark API calls as coming from our own client (CSRF guard).
		const headers = { 'X-PixelPlus-Request': '1', ...extra };
		if (cookie) headers.cookie = cookie;
		let payload;
		if (body instanceof FormData) payload = body;
		else if (body !== undefined) {
			payload = JSON.stringify(body);
			headers['content-type'] = 'application/json';
		}
		const r = await fetch(base + p, { method, headers, body: payload });
		const setCookie = r.headers.get('set-cookie');
		if (setCookie) cookie = setCookie.split(';')[0].endsWith('=') ? '' : setCookie.split(';')[0];
		if (raw) {
			if (expect !== null && r.status !== expect)
				throw new Fail(`${method} ${node}${p} → ${r.status}, expected ${expect}`);
			return r;
		}
		const text = await r.text();
		let data;
		try {
			data = text ? JSON.parse(text) : undefined;
		} catch {
			data = text;
		}
		if (expect !== null && r.status !== expect)
			throw new Fail(`${method} ${node}${p} → ${r.status} ${text.slice(0, 300)} (expected ${expect})`);
		if (r.status >= 400) {
			// Every error must be the documented shape with a human message.
			check(data?.error?.code && data?.error?.message, `${method} ${p}: error body ${text.slice(0, 200)}`);
		}
		return data;
	}
	return {
		base,
		get: (p, o) => call('GET', p, undefined, o),
		post: (p, b = {}, o) => call('POST', p, b, o),
		put: (p, b = {}, o) => call('PUT', p, b, o),
		del: (p, o) => call('DELETE', p, undefined, o),
		raw: (m, p, o) => call(m, p, undefined, { ...o, raw: true }),
		get cookie() {
			return cookie;
		},
		set cookie(c) {
			cookie = c;
		}
	};
}
const L = client('leader');
const F1 = client('f1');
const F2 = client('f2');
const NODES = { leader: L, f1: F1, f2: F2 };

function cluster(...a) {
	const r = spawnSync(path.join(ROOT, 'scripts/dev-cluster.sh'), a, { encoding: 'utf8', env: process.env });
	if (r.status !== 0) throw new Fail(`dev-cluster.sh ${a.join(' ')} failed:\n${r.stdout}\n${r.stderr}`);
	return r.stdout;
}

/** One tapped frame of a node: {nodeId, seq, frame, wallMs, master, player, outputs:[Buffer rgb], wire:[Buffer]} */
async function tap(c) {
	const d = await c.get('/debug/output');
	return {
		...d,
		seq: d.sequence?.id ?? null,
		frame: d.sequence?.frame ?? null,
		// Timeline position (ms) at the moment the frame lights up (wall clock, µs precision).
		posMs: d.posMs ?? null,
		lightWallMs: d.lightWallMs ?? null,
		rgb: d.outputs.map((o) => Buffer.from(o.rgb, 'base64')),
		wire: d.outputs.map((o) => Buffer.from(o.wire, 'base64')),
		ppo: d.outputs.map((o) => o.pixels)
	};
}
const lit = (buf) => buf.some((b) => b !== 0);

// ---------------------------------------------------------------------------
// Scenario state
// ---------------------------------------------------------------------------

const S = {
	ids: {},
	props: {},
	show: null,
	seq: null,
	seq2: null,
	media: null,
	playlist: null,
	fseqBytes: null
};
const chanCache = new Map();
function chanFrame(frame) {
	if (!chanCache.has(frame)) {
		const b = Buffer.alloc(S.channels);
		fillFrame(S.patternProps, frame, FRAME_MS, b);
		chanCache.set(frame, b);
	}
	return chanCache.get(frame);
}
/** Compare a tapped node frame with the expected pixels of sequence frame `t.frame`. */
function verifyPixels(nodeName, t) {
	const expected = nodeFrame(S.show.props, S.ids[nodeName], chanFrame(t.frame), t.ppo);
	const got = Buffer.concat(t.rgb);
	if (!expected.equals(got)) {
		let i = 0;
		while (expected[i] === got[i]) i++;
		throw new Fail(
			`${nodeName}: pixels of frame ${t.frame} differ at byte ${i} (expected ${expected.subarray(i, i + 6).toString('hex')}, got ${got.subarray(i, i + 6).toString('hex')})`
		);
	}
}

// ---------------------------------------------------------------------------
// Phases
// ---------------------------------------------------------------------------

async function phaseSetup() {
	await step('start a fresh 3-node cluster', async () => {
		cluster('start', '--fresh');
		for (const c of Object.values(NODES))
			eq((await c.get('/public/health')).role, 'unconfigured', 'fresh role');
		const sys = await L.get('/system');
		check(sys.needsSetup === true, 'fresh leader needs setup');
	});

	await step('setup wizard: leader + two followers', async () => {
		const bad = await L.post('/system/setup', { role: 'boss' }, { expect: 400 });
		check(/leader/.test(bad.error.message), 'friendly role error');
		const tz = await L.post(
			'/system/setup',
			{ role: 'leader', location: { lat: 0, lon: 0, timezone: 'Mars/Olympus' } },
			{ expect: 400 }
		);
		check(/time zone/.test(tz.error.message), 'bad time zone rejected');
		const ls = await L.post('/system/setup', {
			role: 'leader',
			showName: 'E2E Show',
			name: 'Main',
			board: 'difftxlarge',
			location: { lat: 40.7128, lon: -74.006, timezone: 'America/New_York', label: 'New York' }
		});
		eq([ls.role, ls.needsSetup, ls.showName, ls.name], ['leader', false, 'E2E Show', 'Main'], 'leader setup');
		const f1 = await F1.post('/system/setup', { role: 'follower', name: 'Porch', board: 'difftx' });
		const f2 = await F2.post('/system/setup', { role: 'follower', name: 'Garage', board: 'difftx' });
		eq([f1.role, f2.role, f1.outputs, f2.outputs], ['follower', 'follower', 4, 4], 'follower setup');
		S.ids = { leader: ls.nodeId, f1: f1.nodeId, f2: f2.nodeId };
		const show = await L.get('/show');
		eq(
			show.nodes.map((n) => [n.id, n.role, n.outputs.length]),
			[[ls.nodeId, 'leader', 60]],
			'leader node in show'
		);
		eq(show.schedule.location.timezone, 'America/New_York', 'schedule location from the wizard');
	});

	await step('leader discovers and adopts both followers', async () => {
		const found = await until('both followers in /nodes/discovered', async () => {
			const d = await L.get('/nodes/discovered');
			// Names from the wizard arrive with the next beacon (every 2 s).
			const named = { [S.ids.f1]: 'Porch', [S.ids.f2]: 'Garage' };
			return Object.entries(named).every(([id, name]) => d.some((n) => n.id === id && n.name === name)) && d;
		});
		const f1 = found.find((n) => n.id === S.ids.f1);
		eq([f1.name, f1.board, f1.adoptedBy, f1.http], ['Porch', 'difftx', null, URL_OF.f1], 'beacon of f1');
		await L.post('/nodes/adopt', { id: S.ids.f1 });
		await L.post('/nodes/adopt', { id: S.ids.f2, name: 'Garage' });
		await until('followers online + synced', async () => {
			const n = await L.get('/nodes');
			return (
				n.length === 3 &&
				n.every((x) => x.online && x.adopted) &&
				n.filter((x) => x.role === 'follower').every((x) => x.syncState === 'synced')
			);
		});
		eq((await L.get('/nodes/discovered')).length, 0, 'nothing left to adopt');
		const sys = await F1.get('/system');
		eq(
			[sys.role, sys.leaderName, sys.showName],
			['follower', 'Main', 'E2E Show'],
			'follower knows its leader'
		);
		await until('f1 manifest (show name)', async () => (await F1.get('/show')).name === 'E2E Show');
	});
}

async function phaseImport() {
	await step('xLights import: preview, match controllers, apply', async () => {
		const td = path.join(ROOT, 'crates/pixelplus-core/testdata');
		const fd = new FormData();
		fd.append(
			'rgbeffects',
			new Blob([fs.readFileSync(path.join(td, 'xlights_2025_rgbeffects.xml'))]),
			'xlights_rgbeffects.xml'
		);
		fd.append(
			'networks',
			new Blob([fs.readFileSync(path.join(td, 'xlights_2025_networks.xml'))]),
			'xlights_networks.xml'
		);
		const preview = await L.post('/import/xlights', fd);
		eq(preview.controllers.map((c) => c.name).sort(), ['Front PixelPlus', 'Tree F48'], 'controllers');
		check(preview.props.length >= 9, `props in preview: ${preview.props.length}`);
		check(
			preview.warnings.some((w) => /Window/.test(w)),
			'unassigned model warned'
		);
		const show = await L.post('/import/xlights/apply', {
			preview,
			controllerMap: { 'Front PixelPlus': S.ids.f1, 'Tree F48': S.ids.leader }
		});
		for (const p of show.props) S.props[p.name] = p;
		// "Front PixelPlus" has 5 ports in xLights, the pHAT only 4: port 5 is left unwired.
		const poly = S.props['Garage Poly'];
		eq(
			poly.segments.map((s) => [s.nodeId, s.output]),
			[[S.ids.f1, 4]],
			'Garage Poly wiring after import'
		);
		const h = await L.post('/health/run');
		const wiring = h.checks.find((c) => c.id === 'wiring');
		check(
			wiring.status === 'warn' && /Partly wired: Garage Poly/.test(wiring.detail),
			`wiring check: ${wiring.detail}`
		);
		check(/Window not wired/.test(wiring.detail), `unwired Window: ${wiring.detail}`);
	});

	await step('wire the rest by hand (Garage Poly 2nd half + Window on f2)', async () => {
		const poly = structuredClone(S.props['Garage Poly']);
		poly.segments.push({
			nodeId: S.ids.f2,
			output: 1,
			startPixel: 0,
			pixelCount: 75,
			propOffset: 75,
			reverse: true,
			nullPixels: 0
		});
		// A segment on an output the board doesn't have is refused with a clear message.
		const bad = structuredClone(poly);
		bad.segments[1].output = 5;
		const err = await L.put(`/props/${poly.id}`, bad, { expect: 400 });
		check(/Garage doesn't have output 5/.test(err.error.message), err.error.message);
		await L.put(`/props/${poly.id}`, poly);
		const win = structuredClone(S.props['Window']);
		win.segments = [
			{
				nodeId: S.ids.f2,
				output: 2,
				startPixel: 2,
				pixelCount: 60,
				propOffset: 0,
				reverse: false,
				nullPixels: 2
			}
		];
		await L.put(`/props/${win.id}`, win);
		S.show = await L.get('/show');
		for (const p of S.show.props) S.props[p.name] = p;
		const h = await L.post('/health/run');
		eq(h.checks.find((c) => c.id === 'wiring').status, 'ok', 'wiring ok');
		await until('followers got their props', async () => {
			const [a, b] = await Promise.all([F1.get('/show'), F2.get('/show')]);
			return a.props.length === 5 && b.props.length === 2;
		});
	});
}

async function phaseContent() {
	await step('generate .fseq (zstd) + WAV, upload audio then sequence (auto-link)', async () => {
		S.patternProps = S.show.props;
		S.channels = channelCount(S.show.props);
		S.fseqBytes = writeFseq({
			channelCount: S.channels,
			frameMs: FRAME_MS,
			frames: SONG_FRAMES,
			mediaFilename: 'C:\\Users\\me\\Documents\\xLights\\Audio\\E2E Song.wav',
			frame: (i, b) => fillFrame(S.show.props, i, FRAME_MS, b)
		});
		const wav = writeWav({ seconds: (SONG_FRAMES * FRAME_MS) / 1000 });
		const mf = new FormData();
		mf.append('file', new Blob([wav]), 'E2E Song.wav');
		S.media = await L.post('/media', mf);
		eq([S.media.kind, S.media.durationMs], ['song', 20000], 'media');
		check(typeof S.media.loudnessLufs === 'number', 'loudness measured');
		const sf = new FormData();
		sf.append('fseq', new Blob([S.fseqBytes]), 'E2E Song.fseq');
		S.seq = await L.post('/sequences', sf);
		eq(
			[S.seq.durationMs, S.seq.frameMs, S.seq.channelCount, S.seq.mediaId],
			[20000, 50, S.channels, S.media.id],
			'sequence'
		);
		const th = await L.raw('GET', `/sequences/${S.seq.id}/thumbnail`);
		eq(th.headers.get('content-type'), 'image/png', 'thumbnail type');
		const png = Buffer.from(await th.arrayBuffer());
		eq(png.subarray(1, 4).toString(), 'PNG', 'thumbnail is a PNG');
		const peaks = await L.get(`/media/${S.media.id}/peaks?n=50`);
		check(
			peaks.length === 50 && peaks.every((p) => p >= 0 && p <= 1) && Math.max(...peaks) > 0.1,
			'waveform peaks'
		);
	});

	await step('upload a second sequence together with its audio', async () => {
		const fd = new FormData();
		const short = writeFseq({
			channelCount: S.channels,
			frameMs: 25,
			frames: 200,
			frame: (i, b) => fillFrame(S.show.props, i, 25, b)
		});
		fd.append('fseq', new Blob([short]), 'Short One.fseq');
		fd.append('audio', new Blob([writeWav({ seconds: 5, hz: 330 })]), 'Short One.wav');
		S.seq2 = await L.post('/sequences', fd);
		eq([S.seq2.durationMs, S.seq2.frameMs, S.seq2.name], [5000, 25, 'Short One'], 'second sequence');
		check(S.seq2.mediaId && S.seq2.mediaId !== S.media.id, 'own media linked');
		const bad = new FormData();
		bad.append('fseq', new Blob([Buffer.from('not an fseq at all')]), 'Broken.fseq');
		const err = await L.post('/sequences', bad, { expect: 400 });
		check(/isn't a sequence PixelPlus can play/.test(err.error.message), err.error.message);
	});

	await step('followers receive byte-correct slices', async () => {
		await until(
			'slices on both followers',
			async () => {
				const n = await L.get('/nodes');
				return n
					.filter((x) => x.role === 'follower')
					.every((x) => x.files.total === 2 && x.files.pending === 0 && x.syncState === 'synced');
			},
			{ timeout: 30000 }
		);
		const sha = crypto.createHash('sha256').update(S.fseqBytes).digest('hex');
		for (const f of ['f1', 'f2']) {
			const file = path.join(DIR, f, 'sequences', `${S.seq.id}.ppseq`);
			const s = readPpseq(fs.readFileSync(file));
			eq([s.frameCount, s.frameUs, s.sha], [SONG_FRAMES, FRAME_MS * 1000, sha], `${f} slice header`);
			for (let i = 0; i < s.frameCount; i++) {
				const exp = nodeFrame(S.show.props, S.ids[f], chanFrame(i), s.ppo);
				if (!exp.equals(s.frame(i)))
					throw new Fail(`${f}: slice frame ${i} differs from the leader's rendering`);
			}
			log(`${f}: ${s.frameCount} frames, outputs ${s.ppo.join('/')} px — all bytes match`);
		}
		check(
			!fs.existsSync(path.join(DIR, 'leader', 'sequences', `${S.seq.id}.ppseq`)),
			'leader plays the fseq itself'
		);
	});
}

async function phaseShow() {
	await step('playlist (sequence, DJ, pause, look) + schedule entry active now', async () => {
		const clip = await L.post('/dj-clips', {
			name: 'Welcome',
			lines: [{ voice: 'nick', text: 'Welcome to the E2E show!', pauseMs: 0 }],
			dynamic: false,
			speed: 1
		});
		const fd = new FormData();
		fd.append('audio', new Blob([writeWav({ seconds: 2, hz: 660 })]), `${clip.id}.wav`);
		const up = await L.post(`/dj-clips/${clip.id}/upload`, fd);
		check(up.mediaId, 'browser-rendered DJ audio stored');
		const looks = await L.get('/effects');
		const look = looks.find((e) => e.effect === 'candycane') ?? looks[0];
		const pl =
			(await L.get('/playlists'))[0] ?? (await L.post('/playlists', { name: 'Main Show', items: [] }));
		pl.items = [
			{ id: 'it1', type: 'sequence', sequenceId: S.seq.id },
			{ id: 'it2', type: 'dj', djClipId: clip.id },
			{ id: 'it3', type: 'pause', durationMs: 1500 },
			{ id: 'it4', type: 'effect', effectId: look.id, durationMs: 3000 }
		];
		pl.repeat = true;
		S.playlist = await L.put(`/playlists/${pl.id}`, pl);
		S.look = look;
		const sch = await L.get('/schedule');
		const now = new Date(new Date().toLocaleString('en-US', { timeZone: sch.location.timezone }));
		const hm = (d) => `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
		const start = new Date(+now - 2 * 60000);
		const end = new Date(+now + 90 * 60000);
		if (end.getDate() !== start.getDate()) log('note: the show window crosses midnight');
		sch.entries = [
			{
				id: 'e2e-now',
				name: 'E2E tonight',
				enabled: true,
				playlistId: pl.id,
				days: ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'],
				start: { kind: 'clock', time: hm(start) },
				end: { kind: 'clock', time: hm(end) },
				priority: 1,
				endBehavior: 'stopNow'
			}
		];
		sch.enabled = true;
		await L.put('/schedule', sch);
		const prev = await L.get('/schedule/preview?days=2');
		check(
			prev.some((o) => o.entryId === 'e2e-now'),
			'schedule preview lists the entry'
		);
	});

	await step('scheduler starts the show; WebSocket status progresses', async () => {
		const st = await until(
			'player playing the sequence',
			async () => {
				const p = await L.get('/player');
				return p.state === 'playing' && p.item?.id === S.seq.id && p;
			},
			{ timeout: 20000 }
		);
		eq([st.playlist?.id, st.scheduleEntry?.id, st.durationMs], [S.playlist.id, 'e2e-now', 20000], 'status');
		const ws = new WebSocket(`ws://127.0.0.1:${URL_OF.leader}/api/v1/ws`);
		const msgs = [];
		ws.onmessage = (e) => typeof e.data === 'string' && msgs.push(JSON.parse(e.data));
		await sleep(2000);
		ws.close();
		const statuses = msgs.filter((m) => m.type === 'status').map((m) => m.data);
		check(statuses.length >= 4, `status messages while playing: ${statuses.length}`);
		const pos = statuses.filter((s) => s.item?.id === S.seq.id).map((s) => s.posMs);
		check(pos.length >= 2 && pos.at(-1) > pos[0], `posMs advances: ${pos.join(',')}`);
		check(
			msgs.some((m) => m.type === 'nodes'),
			'nodes message on connect'
		);
		for (const f of [F1, F2]) {
			const p = await f.get('/player');
			eq([p.state, p.item?.id], ['playing', S.seq.id], 'follower plays too');
		}
	});

	await step('frame-accurate sync: all three nodes show the right pixels (±1 frame)', async () => {
		let worst = 0;
		for (let k = 0; k < 12; k++) {
			const taps = await Promise.all([tap(L), tap(F1), tap(F2)]);
			const [l, a, b] = taps;
			if (taps.some((t) => t.seq !== S.seq.id)) {
				check(k > 0, `not all nodes are on the sequence: ${taps.map((t) => t.seq).join(',')}`);
				break; // the song ended
			}
			for (const [name, t] of [
				['f1', a],
				['f2', b]
			]) {
				// Frame difference corrected for when each snapshot was taken.
				const drift = t.frame - l.frame - (t.wallMs - l.wallMs) / FRAME_MS;
				worst = Math.max(worst, Math.abs(drift));
				check(Math.abs(drift) <= 1.5, `${name} is ${drift.toFixed(2)} frames off the leader`);
			}
			verifyPixels('leader', l);
			verifyPixels('f1', a);
			verifyPixels('f2', b);
			// The marker pixel of Big Arch (f1 output 1, first pixel) carries the frame number.
			eq([a.rgb[0][0], a.rgb[0][1], a.rgb[0][2]], [a.frame & 255, a.frame >> 8, MARKER_B], 'marker pixel');
			await sleep(300);
		}
		log(`worst follower offset ${worst.toFixed(2)} frames`);
	});

	await step('timeline sync in milliseconds: followers within 2 ms of the leader', async () => {
		// Every tap says which timeline position (posMs) its node shows at which
		// wall-clock instant (lightWallMs, µs precision). All nodes share this
		// machine's clock, so (posMs − lightWallMs) differences are the timeline
		// offsets in ms, free of the ±1-frame quantisation of frame numbers.
		const measure = async (label, n) => {
			const offs = { f1: [], f2: [] };
			for (let k = 0; k < n; k++) {
				const [l, a, b] = await Promise.all([tap(L), tap(F1), tap(F2)]);
				if ([l, a, b].some((t) => t.seq !== S.seq.id || t.posMs == null)) break;
				const base = l.posMs - l.lightWallMs;
				offs.f1.push(a.posMs - a.lightWallMs - base);
				offs.f2.push(b.posMs - b.lightWallMs - base);
				await sleep(100);
			}
			check(offs.f1.length >= Math.min(10, n), `${label}: only ${offs.f1.length} samples`);
			for (const f of ['f1', 'f2']) {
				const xs = offs[f];
				const abs = xs.map(Math.abs).sort((x, y) => x - y);
				const mean = xs.reduce((s, x) => s + x, 0) / xs.length;
				const p95 = abs[Math.floor(abs.length * 0.95)];
				const worst = abs.at(-1);
				log(
					`${label} ${f}: mean ${mean.toFixed(3)} ms, p95 |err| ${p95.toFixed(3)} ms, worst ${worst.toFixed(3)} ms (${xs.length} samples)`
				);
				if (process.env.PP_E2E_SYNC_MS !== 'off')
					check(worst < Number(process.env.PP_E2E_SYNC_MS ?? 2), `${label} ${f} timeline error ${worst.toFixed(3)} ms`);
			}
		};
		// While playing on from the scheduled start…
		await measure('playing', 15);
		// …and after a seek (followers jump, then converge).
		await L.post('/player/seek', { posMs: 2000 });
		await sleep(1500);
		await measure('after seek', 30);
		// Each follower reports its timing quality to the leader (Controllers page badge).
		for (const n of (await L.get('/nodes')).filter((x) => x.role === 'follower')) {
			eq(n.protocol, 2, `${n.name} cluster protocol`);
			check(n.sync && n.sync.samples > 0, `${n.name} reports sync quality: ${JSON.stringify(n.sync)}`);
			check(n.sync.offsetErrorMs < 1, `${n.name} clock error bound ${n.sync.offsetErrorMs} ms`);
			check(n.sync.lossPct <= 10, `${n.name} lost ${n.sync.lossPct} % of clock probes`);
			log(
				`${n.name}: clock ±${n.sync.offsetErrorMs} ms, RTT ${n.sync.rttMs}/${n.sync.rttP50Ms}/${n.sync.rttP95Ms} ms, ` +
					`drift ${n.sync.driftPpm} ppm, kernel stamps ${n.sync.kernelTimestamps}`
			);
		}
	});

	await step('seek, pause/resume keep the followers in step', async () => {
		await L.post('/player/seek', { posMs: 12000 });
		await sleep(700);
		const [l, a] = await Promise.all([tap(L), tap(F1)]);
		check(l.frame >= 240 && l.frame < 260, `leader frame after seek ${l.frame}`);
		check(
			Math.abs(a.frame - l.frame - (a.wallMs - l.wallMs) / FRAME_MS) <= 1.5,
			`f1 after seek ${a.frame} vs ${l.frame}`
		);
		await L.post('/player/pause');
		await sleep(600);
		const p1 = await Promise.all([tap(L), tap(F2)]);
		await sleep(600);
		const p2 = await Promise.all([tap(L), tap(F2)]);
		eq([p2[0].frame, p2[1].frame], [p1[0].frame, p1[1].frame], 'paused frames hold');
		check(Math.abs(p2[0].frame - p2[1].frame) <= 1, `paused on the same frame ${p2[0].frame}/${p2[1].frame}`);
		eq((await F2.get('/player')).state, 'paused', 'follower paused');
		await L.post('/player/resume');
		await until('playing again', async () => (await F2.get('/player')).state === 'playing', {
			timeout: 3000
		});
	});

	await step('playlist moves on: DJ clip → pause → look (followers run the look)', async () => {
		await L.post('/player/seek', { posMs: 19000 });
		await until('DJ clip', async () => (await L.get('/player')).item?.type === 'dj', { timeout: 8000 });
		await until('pause', async () => (await L.get('/player')).item?.type === 'pause', { timeout: 8000 });
		await until('look', async () => (await L.get('/player')).item?.type === 'effect', { timeout: 8000 });
		await sleep(400);
		const [l, a, b] = await Promise.all([tap(L), tap(F1), tap(F2)]);
		check(
			lit(Buffer.concat(l.rgb)) && lit(Buffer.concat(a.rgb)) && lit(Buffer.concat(b.rgb)),
			'look lights every node'
		);
		eq((await F1.get('/player')).state, (await L.get('/player')).state, 'follower state follows the leader');
		await until('back to the sequence (repeat)', async () => (await L.get('/player')).item?.id === S.seq.id, {
			timeout: 8000
		});
	});
}

async function phaseTools() {
	const lightsOf = async (c) => Buffer.concat((await tap(c)).rgb);
	await step('test patterns reach every node, stop restores the show', async () => {
		await L.post('/test/start', { mode: 'solid', color: '#ff0000', target: { all: true } });
		await sleep(600);
		for (const [n, c] of Object.entries(NODES)) {
			const t = await tap(c);
			const px = Buffer.concat(t.rgb);
			const pixels = [];
			for (let i = 0; i < px.length; i += 3)
				if (px[i] || px[i + 1] || px[i + 2]) pixels.push(px.subarray(i, i + 3).toString('hex'));
			check(pixels.length > 0, `${n}: test pattern lit nothing`);
			check(
				pixels.every((p) => p === 'ff0000'),
				`${n}: not solid red (${[...new Set(pixels)].slice(0, 4)})`
			);
		}
		eq((await L.get('/player')).state, 'testing', 'leader state testing');
		// Raw output test on one follower port.
		await L.post('/test/start', { mode: 'solid', color: '#00ff00', target: { nodeId: S.ids.f2, output: 2 } });
		await sleep(600);
		const t = await tap(F2);
		check(
			t.rgb[1].length && t.rgb[1].every((v, i) => (i % 3 === 1 ? v === 255 : v === 0)),
			'f2 output 2 all green'
		);
		await L.post('/test/stop');
		await until('show back after the test', async () => (await L.get('/player')).state !== 'testing', {
			timeout: 3000
		});
	});

	await step('identify a follower + the leader', async () => {
		await L.post(`/nodes/${S.ids.f1}/identify`);
		await L.post(`/nodes/${S.ids.leader}/identify`);
	});

	await step('fault finder walks a follower prop to the bad pixel', async () => {
		const arch = S.props['Big Arch'];
		let st = await L.post('/faultfinder/start', { propId: arch.id });
		eq([st.litFrom, st.litTo, st.done], [0, 50, false], 'first step');
		await sleep(500);
		const t = await tap(F1);
		const out1 = t.rgb[0];
		check(
			lit(out1.subarray(0, 50 * 3)) && !lit(out1.subarray(50 * 3)),
			'f1 lights exactly pixels 1–50 of Big Arch'
		);
		// Pretend pixel 38 (index 37) is broken: "yes" while all lit pixels are before it.
		let guard = 0;
		while (!st.done && guard++ < 12)
			st = await L.post(`/faultfinder/${st.session}/answer`, { lit: st.litTo <= 37 });
		check(st.done && st.result, 'fault finder finished');
		eq(st.result.pixelIndex, 37, 'found pixel');
		await L.post('/faultfinder/stop');
	});

	await step('blackout and brightness propagate to followers', async () => {
		await L.post('/player/blackout', { enabled: true });
		await sleep(500);
		for (const [n, c] of Object.entries(NODES)) {
			const t = await tap(c);
			check(!lit(Buffer.concat(t.wire)), `${n}: output not dark in blackout`);
		}
		check((await L.get('/player')).blackout === true, 'status blackout');
		await L.post('/player/blackout', { enabled: false });
		await L.put('/player/brightness', { brightness: 40 });
		await sleep(600);
		for (const [n, c] of Object.entries(NODES)) eq((await tap(c)).master, 40, `${n} master brightness`);
		await L.put('/player/brightness', { brightness: 100 });
		const bad = await L.put('/player/brightness', { brightness: 'loud' }, { expect: 400 });
		check(bad.error.message, 'bad brightness explained');
	});

	await step('live look on selected props, then stop', async () => {
		const look = structuredClone(S.look);
		look.target = { all: false, propIds: [S.props['Big Arch'].id], groupIds: [] };
		look.params = { color: '#0000ff' };
		look.effect = 'solid';
		await L.post('/player/effect', { effect: look });
		await sleep(600);
		eq((await L.get('/player')).state, 'effect', 'effect state');
		const t = await tap(F1);
		const arch = t.rgb[0].subarray(0, 300);
		check(
			lit(arch) && [...arch].every((v, i) => (i % 3 === 2 ? v > 0 : v === 0)),
			'Big Arch solid blue on f1'
		);
		check(!lit(t.rgb[1]), 'Canes (not targeted) dark');
		await L.post('/player/effect', { effect: null });
	});

	await step('overlays: text on the matrix, friendly QR/size errors', async () => {
		const m = S.props['Matrix'];
		check(m.matrix?.width > 0, 'Matrix has matrix geometry');
		await L.post(`/overlay/${m.id}/text`, { text: 'HELLO', color: '#ff0000', durationMs: 2000 });
		const qr = await L.post(
			`/overlay/${m.id}/qr`,
			{ url: 'http://pixelplus.local/request', durationMs: 2000 },
			{ expect: null }
		);
		check(qr.ok || /QR code/.test(qr.error?.message ?? ''), `QR answer: ${JSON.stringify(qr)}`);
		const nm = await L.post(`/overlay/${S.props['Big Arch'].id}/text`, { text: 'x' }, { expect: 400 });
		check(/matrix/.test(nm.error.message), nm.error.message);
		await L.post(`/overlay/${m.id}`, { enabled: false });
	});

	await step('song requests: public page, submit, admin queue', async () => {
		await L.put('/show/settings', {
			requests: { enabled: true, maxQueue: 5, title: 'Pick a song', message: 'Tune to 88.1' }
		});
		const pub = await L.get('/public/requests');
		eq([pub.enabled, pub.title], [true, 'Pick a song'], 'public request page');
		check(
			pub.songs.some((s) => s.sequenceId === S.seq.id),
			'songs listed'
		);
		const r = await L.post('/public/requests', { sequenceId: S.seq2.id, name: 'Ana' });
		check(r.ok && r.position >= 1, 'request accepted');
		const q = await L.get('/requests');
		check(
			q.some((x) => x.sequenceId === S.seq2.id),
			'admin queue has it'
		);
		await L.del(`/requests/${q[0].id}`);
		const bad = await L.post('/public/requests', { sequenceId: 'nope' }, { expect: null });
		check(bad.error, 'unknown song refused');
	});

	await step('snapshots: create, change, restore, delete', async () => {
		const snap = await L.post('/snapshots', { label: 'Before e2e rename' });
		check(snap.id && snap.sizeBytes > 0, 'snapshot created');
		await L.put('/show/name', { name: 'Renamed Show' });
		eq((await L.get('/show')).name, 'Renamed Show', 'renamed');
		await L.post(`/snapshots/${snap.id}/restore`);
		eq((await L.get('/show')).name, 'E2E Show', 'restored name');
		const dl = await L.raw('GET', `/snapshots/${snap.id}/download`);
		check((await dl.arrayBuffer()).byteLength > 0, 'download');
		const list = await L.get('/snapshots');
		check(
			list.some((s) => s.id === snap.id),
			'listed'
		);
		await L.del(`/snapshots/${snap.id}`);
		await until(
			'followers back on the restored show',
			async () => (await F1.get('/show')).name === 'E2E Show'
		);
	});

	await step('health, power estimate, sensors', async () => {
		await until('followers synced', async () =>
			(await L.get('/nodes')).every((n) => n.syncState === 'synced')
		);
		const h = await L.post('/health/run');
		const ids = h.checks.map((c) => c.id);
		for (const id of ['followers', 'sync', 'disk', 'audio', 'output', 'clock', 'sequences', 'wiring', 'schedule'])
			check(ids.includes(id), `health check ${id}`);
		const timing = h.checks.find((c) => c.id === 'sync');
		check(timing.status === 'ok' && /within ±/.test(timing.detail), `timing check: ${timing.detail}`);
		const audio = h.checks.find((c) => c.id === 'audio');
		check(
			audio.status === 'warn' && /turned off/.test(audio.detail),
			`audio check with PIXELPLUS_AUDIO=none: ${audio.detail}`
		);
		eq(h.checks.find((c) => c.id === 'followers').status, 'ok', 'controllers ok');
		const p = await L.get(`/power/estimate?sequenceId=${S.seq.id}`);
		check(p.perOutput.length > 20 && p.perProp.length === S.show.props.length, 'power per output / prop');
		check(
			p.perOutput.every((o) => o.peakAmps >= o.avgAmps),
			'peak ≥ average'
		);
		const sensors = await until(
			'simulated sensors (PIXELPLUS_DEV)',
			async () => {
				const s = await L.get('/system/sensors');
				return s.some((x) => x.id === 'inputVoltage') && s;
			},
			{ timeout: 12000 }
		);
		check(
			sensors.every((s) => typeof s.value === 'number' && s.unit && s.nodeId),
			'sensor shape'
		);
		const hist = await L.get('/system/sensors/history?minutes=10');
		check(Object.keys(hist.series).length > 0, 'sensor history');
	});

	await step('sync lights to sound: calibration flashes on every controller, delay moves the lights', async () => {
		const before = (await L.get('/show')).settings.audio.outputDelayMs ?? 0;
		await L.post('/player/calibration', { on: true });
		await until('calibration running everywhere', async () =>
			(await Promise.all([L, F1, F2].map((c) => c.get('/player')))).every((p) => p.item?.type === 'calibration')
		);
		// Every controller flashes white in the same 50 ms after each whole second.
		const flashed = new Set();
		const t0 = Date.now();
		while (flashed.size < 3 && Date.now() - t0 < 6000) {
			const taps = await Promise.all([tap(L), tap(F1), tap(F2)]);
			taps.forEach((t, i) => {
				if (t.rgb.some((o) => o.length && o.every((b) => b === 255))) flashed.add(i);
			});
			await sleep(5);
		}
		eq(flashed.size, 3, 'all three controllers flashed');
		// The sound delay shifts the lights timeline on every node at once.
		const pos = async () => {
			const [l, a] = await Promise.all([tap(L), tap(F1)]);
			return [l.posMs - l.lightWallMs, a.posMs - a.lightWallMs];
		};
		const [l0, f0] = await pos();
		await L.put('/show/settings', { audio: { outputDelayMs: 250 } });
		await sleep(1200);
		const [l1, f1] = await pos();
		check(Math.abs(l0 - l1 - 250) < 30, `leader lights moved by ${(l0 - l1).toFixed(1)} ms`);
		check(Math.abs(f0 - f1 - 250) < 30, `follower lights moved by ${(f0 - f1).toFixed(1)} ms`);
		check(Math.abs(l1 - f1) < 2, `follower within ${Math.abs(l1 - f1).toFixed(3)} ms after the change`);
		await L.put('/show/settings', { audio: { outputDelayMs: before } });
		await L.post('/player/calibration', { on: false });
		await until('calibration stopped', async () => (await L.get('/player')).item?.type !== 'calibration');
	});

	await step('games and TTS without their sidecars degrade gracefully', async () => {
		const g = await L.get('/games/status');
		eq([g.running, g.available], [false, false], 'games not running');
		const inv = await L.post('/games/invite', {}, { expect: null });
		check(
			inv?.error?.message && !/panic|internal/i.test(inv.error.message),
			`games invite: ${JSON.stringify(inv)}`
		);
		const t = await L.get('/tts/status');
		eq([t.mode, t.available], ['browser', true], 'TTS falls back to the browser');
	});

	await step('password: set, 401 without session, login/logout, cluster keeps working', async () => {
		await L.put('/auth/password', { password: 'e2e-secret' });
		await L.get('/show', { expect: 401 });
		eq((await L.get('/public/health')).ok, true, 'public health stays open');
		await L.get('/public/requests');
		const sys = await L.get('/system');
		check(
			sys.passwordSet === true && sys.ips === undefined && sys.cpuPct === undefined,
			'unauthenticated /system is minimal'
		);
		const wrong = await L.post('/auth/login', { password: 'nope' }, { expect: 401 });
		check(/isn't right/.test(wrong.error.message), wrong.error.message);
		await L.post('/auth/login', { password: 'e2e-secret' });
		check(L.cookie.startsWith('pp_session='), 'session cookie');
		await L.get('/show');
		// Followers still sync with the cluster key.
		await L.put('/player/brightness', { brightness: 90 });
		await until('f1 brightness 90', async () => (await tap(F1)).master === 90, { timeout: 3000 });
		await L.put('/player/brightness', { brightness: 100 });
		await L.post('/auth/logout');
		await L.get('/show', { expect: 401 });
		// A sign-in through a tunnel on this machine (loopback + forwarding header) raises an alert.
		const logFile = path.join(DIR, 'leader.log');
		const logBefore = fs.readFileSync(logFile, 'utf8').length;
		await L.post('/auth/login', { password: 'e2e-secret' }, { headers: { 'X-Forwarded-For': '203.0.113.50' } });
		await until('remote sign-in alert', async () =>
			/Remote sign-in: Someone signed in to PixelPlus from 203\.0\.113\.50 through a tunnel or reverse proxy/.test(
				fs.readFileSync(logFile, 'utf8').slice(logBefore)
			)
		);
		await L.post('/auth/logout');
		await L.post('/auth/login', { password: 'e2e-secret' });
		check(
			!/Remote sign-in: Someone signed in to PixelPlus from 127\.0\.0\.1/.test(fs.readFileSync(logFile, 'utf8')),
			'no alert for a local sign-in'
		);
		await L.put('/auth/password', { current: 'e2e-secret', password: null });
		L.cookie = '';
		await L.get('/show');
	});
}

async function phaseResilience() {
	const inSync = async (c, name) => {
		const [l, t] = await Promise.all([tap(L), tap(c)]);
		if (l.seq !== S.seq.id || t.seq !== S.seq.id) return false;
		const drift = t.frame - l.frame - (t.wallMs - l.wallMs) / FRAME_MS;
		if (Math.abs(drift) > 1.5) return false;
		verifyPixels(name, t);
		return true;
	};
	await step('restart a follower mid-show: it resyncs', async () => {
		await L.post('/player/play', { sequenceId: S.seq.id });
		await sleep(1000);
		cluster('restart', 'f1');
		const t0 = Date.now();
		await until('f1 back in sync', () => inSync(F1, 'f1'), { timeout: 10000, every: 200 });
		log(`f1 resynced ${Date.now() - t0} ms after it answered again`);
	});

	await step('live prop / output changes reach the follower', async () => {
		const arch = structuredClone(S.props['Big Arch']);
		arch.name = 'Big Arch (renamed)';
		await L.put(`/props/${arch.id}`, arch);
		await L.put(`/nodes/${S.ids.f1}/outputs/2`, { colorOrder: 'GRB', brightness: 50 });
		await until('f1 show updated', async () => {
			const s = await F1.get('/show');
			const n = s.nodes.find((x) => x.id === S.ids.f1);
			return s.props.some((p) => p.name === 'Big Arch (renamed)') && n.outputs[1].colorOrder === 'GRB';
		});
		await L.post('/player/play', { sequenceId: S.seq.id });
		await sleep(800);
		const t = await tap(F1);
		// Output 2 (Canes): wire order GRB at 50 %: G,R,B of the rendered colour, halved.
		const rgb = t.rgb[1].subarray(3, 6);
		const wire = t.wire[1].subarray(3, 6);
		const half = (v) => Math.round(v / 2);
		check(
			Math.abs(wire[0] - half(rgb[1])) <= 1 &&
				Math.abs(wire[1] - half(rgb[0])) <= 1 &&
				Math.abs(wire[2] - half(rgb[2])) <= 1,
			`GRB 50 %: rgb ${rgb.toString('hex')} → wire ${wire.toString('hex')}`
		);
		arch.name = 'Big Arch';
		await L.put(`/props/${arch.id}`, arch);
		await L.put(`/nodes/${S.ids.f1}/outputs/2`, { colorOrder: 'RGB', brightness: 100 });
	});

	await step('leader crash: followers hold, fade to dark, rejoin when it returns', async () => {
		await L.post('/player/play', { sequenceId: S.seq.id });
		await sleep(1000);
		cluster('kill', 'leader');
		const t0 = Date.now();
		// Followers play on to the end of the item, hold, then fade out.
		await until(
			'followers dark',
			async () => {
				const [a, b] = await Promise.all([tap(F1), tap(F2)]);
				return !lit(Buffer.concat(a.wire)) && !lit(Buffer.concat(b.wire));
			},
			{ timeout: 40000, every: 500 }
		);
		log(`followers dark ${((Date.now() - t0) / 1000).toFixed(1)} s after the leader died`);
		const p = await F1.get('/player');
		check(p.state === 'idle' || p.state === 'stopped', `follower state after losing the leader: ${p.state}`);
		cluster('restart', 'leader');
		await until('followers online again', async () => (await L.get('/nodes')).every((n) => n.online), {
			timeout: 15000
		});
		// The schedule is still active: the leader restarts the show by itself.
		await until('show running again', async () => (await L.get('/player')).state === 'playing', {
			timeout: 20000
		});
		await until('f2 in sync again', () => inSync(F2, 'f2'), { timeout: 15000 });
	});

	await step('remove a follower, re-adopt it', async () => {
		const res = await L.del(`/nodes/${S.ids.f2}?force=1`);
		check(res.ok !== false, 'deleted');
		await until('f2 released', async () => (await F2.get('/system')).leaderName == null, { timeout: 8000 });
		const show = await L.get('/show');
		check(!show.nodes.some((n) => n.id === S.ids.f2), 'f2 gone from the show');
		check(
			show.props.every((p) => p.segments.every((s) => s.nodeId !== S.ids.f2)),
			'its wiring is gone'
		);
		await until(
			'f2 discovered again',
			async () => (await L.get('/nodes/discovered')).some((n) => n.id === S.ids.f2),
			{ timeout: 10000 }
		);
		await L.post('/nodes/adopt', { id: S.ids.f2, name: 'Garage' });
		await until('f2 online', async () =>
			(await L.get('/nodes')).some((n) => n.id === S.ids.f2 && n.online && n.adopted)
		);
		eq((await F2.get('/system')).leaderName, 'Main', 'f2 follows Main again');
		// Wire it again so later runs (and the UI walk) see a complete show.
		const cur = await L.get('/show');
		const poly = cur.props.find((p) => p.name === 'Garage Poly');
		poly.segments.push({
			nodeId: S.ids.f2,
			output: 1,
			startPixel: 0,
			pixelCount: 75,
			propOffset: 75,
			reverse: true,
			nullPixels: 0
		});
		await L.put(`/props/${poly.id}`, poly);
		const win = cur.props.find((p) => p.name === 'Window');
		win.segments = [
			{
				nodeId: S.ids.f2,
				output: 2,
				startPixel: 2,
				pixelCount: 60,
				propOffset: 0,
				reverse: false,
				nullPixels: 2
			}
		];
		await L.put(`/props/${win.id}`, win);
		await until(
			'f2 slices again',
			async () => {
				const n = (await L.get('/nodes')).find((x) => x.id === S.ids.f2);
				return n.files.total === 2 && n.files.pending === 0;
			},
			{ timeout: 20000 }
		);
	});
}


// ---------------------------------------------------------------------------
// Engine features (WS3): countdown (F4), power limiter + dimming (F12),
// surprises (F20), phone calibration v2 (F1), identify test mode (F9).
// ---------------------------------------------------------------------------

async function phaseEngine() {
	const statusOf = async (c) => c.get('/player');
	const whiteLook = async () => {
		const look = await L.post('/effects', {
			name: 'E2E white',
			effect: 'solid',
			params: { color: '#ffffff' },
			target: { all: true, propIds: [], groupIds: [] }
		});
		S.white = look;
		return look;
	};

	await step('countdown intro: every controller counts down, the first song starts on zero', async () => {
		const pl = await L.post('/playlists', {
			name: 'E2E countdown',
			intro: [{ id: 'cd1', type: 'countdown', durationMs: 4000, others: 'fill', finale: 'flash' }],
			items: [{ id: 'cs1', type: 'sequence', sequenceId: S.seq.id }],
			outro: [],
			shuffle: false,
			repeat: false,
			crossfadeMs: 2000
		});
		await L.post('/player/play', { playlistId: pl.id });
		const t0 = Date.now();
		await until(
			'countdown on every controller',
			async () =>
				(await Promise.all([L, F1, F2].map(statusOf))).every((p) => p.item?.type === 'countdown'),
			{ timeout: 3000, every: 50 }
		);
		// Halfway, the "fill up" bar lights part of every controller.
		await sleep(2200);
		for (const [n, c] of Object.entries(NODES)) check(lit(Buffer.concat((await tap(c)).rgb)), `${n}: countdown dark`);
		const song = await until(
			'the first song',
			async () => {
				const p = await statusOf(L);
				return p.item?.type === 'sequence' && p;
			},
			{ timeout: 5000, every: 20 }
		);
		const took = Date.now() - t0;
		check(took > 3300 && took < 4800, `the song started ${took} ms after a 4 s countdown`);
		check(song.item.id === S.seq.id, 'the playlist’s first song');
		await L.post('/player/stop');
	});

	await step('power limiter: a small supply dims only its output; warn mode only reports', async () => {
		await whiteLook();
		await L.post('/player/effect', { effectId: S.white.id });
		await sleep(700);
		const before = await tap(L);
		const out = before.wire.findIndex((w) => w.length && w.every((b) => b === 255));
		check(out >= 0, 'a fully white leader output');
		const pixels = before.ppo[out];
		// Half of what the output draws at white, after the 0.9 safety factor.
		const amps = (pixels * 0.06 * 0.5) / 0.9;
		await L.put('/show/settings', { power: { mode: 'limit', safety: 0.9 } });
		const psu = await L.post('/power-supplies', {
			name: 'E2E tiny PSU',
			volts: 12,
			amps,
			receiverIds: [],
			directOutputs: [{ nodeId: S.ids.leader, output: out + 1 }]
		});
		const dup = await L.post(
			'/power-supplies',
			{ name: 'Twice', volts: 12, amps: 5, receiverIds: [], directOutputs: [{ nodeId: S.ids.leader, output: out + 1 }] },
			{ expect: 400 }
		);
		check(/already fed/.test(dup.error.message), dup.error.message);
		const t = await until(
			'the supply’s output dimmed to about half',
			async () => {
				const t = await tap(L);
				const w = t.wire[out];
				return w.every((b) => b > 110 && b < 146) && t;
			},
			{ timeout: 6000 }
		);
		check(t.wire.some((w, i) => i !== out && w.length && w.every((b) => b === 255)), 'other outputs untouched');
		const st = await statusOf(L);
		check(st.power?.limiting && st.power.minScale < 0.6, `status.power ${JSON.stringify(st.power)}`);
		const live = await L.get('/power/live');
		const g = live.nodes.find((n) => n.nodeId === S.ids.leader)?.groups.find((x) => x.id === `supply:${psu.id}`);
		check(g && g.scale < 0.6 && g.budget > 0, `live group ${JSON.stringify(g)}`);
		const budget = await L.get(`/power/budget?nodeId=${S.ids.leader}`);
		check(budget.groups.some((x) => x.id === `supply:${psu.id}` && x.tauMs === 0), 'budget has the supply');
		await L.put('/show/settings', { power: { mode: 'warn' } });
		await until('warn mode: full white again', async () => (await tap(L)).wire[out].every((b) => b === 255), {
			timeout: 6000
		});
		check((await statusOf(L)).power?.limiting, 'warn mode still reports');
		await L.del(`/power-supplies/${psu.id}`);
		// A follower's limiter: its report reaches the leader (/nodes limiter, /power/live).
		await L.put('/show/settings', { power: { mode: 'limit' } });
		const ft = await tap(F1);
		const fout = ft.wire.findIndex((w) => w.length && w.every((b) => b === 255));
		check(fout >= 0, 'a fully white follower output');
		const fpsu = await L.post('/power-supplies', {
			name: 'E2E porch PSU',
			volts: 12,
			amps: (ft.ppo[fout] * 0.06 * 0.5) / 0.9,
			receiverIds: [],
			directOutputs: [{ nodeId: S.ids.f1, output: fout + 1 }]
		});
		await until('the follower output dimmed', async () => (await tap(F1)).wire[fout].every((b) => b > 110 && b < 146), {
			timeout: 8000
		});
		await until(
			'the leader sees the follower limiting',
			async () => {
				const n = (await L.get('/nodes')).find((x) => x.id === S.ids.f1);
				const lv = (await L.get('/power/live')).nodes.find((x) => x.nodeId === S.ids.f1);
				return (
					n?.limiter?.minScale < 0.6 &&
					n.limiter.activeGroups.includes(`supply:${fpsu.id}`) &&
					lv?.limiting &&
					lv.groups.some((g) => g.id === `supply:${fpsu.id}` && g.scale < 0.6)
				);
			},
			{ timeout: 10000 }
		);
		check(!(await L.get('/nodes')).find((x) => x.id === S.ids.f2)?.limiter?.activeGroups?.length, 'f2 not limiting');
		await L.del(`/power-supplies/${fpsu.id}`);
		await L.put('/show/settings', { power: { mode: 'warn' } });
	});

	await step('late-night dimming lowers the brightness on every controller', async () => {
		await L.put('/show/settings', {
			power: {
				dim: [{ from: { kind: 'clock', time: '00:00' }, to: { kind: 'clock', time: '00:00' }, brightness: 30, days: [] }]
			}
		});
		await until(
			'every controller at 30 %',
			async () => (await Promise.all([L, F1, F2].map(tap))).every((t) => t.master === 30),
			{ timeout: 6000 }
		);
		eq((await statusOf(L)).brightness, 100, 'the owner’s brightness setting is unchanged');
		await L.put('/show/settings', { power: { dim: [] } });
		await until('back to 100 %', async () => (await tap(F1)).master === 100, { timeout: 6000 });
		await L.post('/player/effect', { effect: null });
	});

	await step('surprise: a look layered over the song on one prop, on its follower; trigger gates', async () => {
		const red = await L.post('/effects', {
			name: 'E2E surprise red',
			effect: 'solid',
			params: { color: '#ff0000' },
			target: { all: true, propIds: [], groupIds: [] }
		});
		await L.post('/player/play', { sequenceId: S.seq.id });
		await until('song playing', async () => (await statusOf(L)).item?.id === S.seq.id);
		const arch = S.props['Big Arch'];
		const r = await L.post('/player/surprise', {
			ref: red.id,
			source: 'effect',
			target: { propIds: [arch.id] },
			durationMs: 3000
		});
		eq(r.surprise.props, 1, 'one prop');
		const archRed = (t) => {
			const a = t.rgb[0].subarray(0, 50 * 3);
			return a.every((v, i) => (i % 3 === 0 ? v === 255 : v === 0));
		};
		await until('Big Arch red on f1', async () => archRed(await tap(F1)), { timeout: 5000, every: 30 });
		eq((await statusOf(L)).item?.id, S.seq.id, 'the song goes on');
		await until('the surprise ends', async () => !archRed(await tap(F1)), { timeout: 5000 });
		// A trigger with a cooldown: fires once, then is gated.
		const show = await L.get('/show');
		const trig = {
			id: 'e2esurprise',
			name: 'E2E doorbell',
			kind: 'http',
			action: { type: 'surprise', ref: red.id, source: 'effect', target: { propIds: [arch.id] }, durationMs: 800 },
			cooldownS: 60
		};
		await L.put('/show/settings', { triggers: [...(show.settings.triggers ?? []), trig] });
		const ok = await L.post(`/triggers/${trig.id}/fire`);
		check(/Surprise/.test(ok.message), ok.message);
		const again = await L.post(`/triggers/${trig.id}/fire`, {}, { expect: 409 });
		check(/cooling down/.test(again.error.message), again.error.message);
		await L.put('/show/settings', { triggers: show.settings.triggers ?? [] });
	});

	await step('phone calibration v2: the seeded pattern flashes on every controller', async () => {
		const cal = await L.post('/player/calibration', { on: true, pattern: 'v2' });
		check(cal.seed > 0 && cal.eventsMs.length >= 32 && cal.flashMs >= 80, `plan ${JSON.stringify(cal).slice(0, 120)}`);
		await until('v2 pattern everywhere', async () =>
			(await Promise.all([L, F1, F2].map(statusOf))).every((p) => p.item?.id === `v2:${cal.seed}`)
		);
		const flashed = new Set();
		const t0 = Date.now();
		while (flashed.size < 3 && Date.now() - t0 < 7000) {
			const taps = await Promise.all([tap(L), tap(F1), tap(F2)]);
			taps.forEach((t, i) => {
				if (t.rgb.some((o) => o.length && o.every((b) => b === 255))) flashed.add(i);
			});
			await sleep(5);
		}
		eq(flashed.size, 3, 'all three controllers flashed');
		await L.post('/player/calibration', { on: false });
		await until('calibration stopped', async () => (await statusOf(L)).item?.type !== 'calibration');
	});

	await step('identify test mode lights one follower output in its colour', async () => {
		await L.post('/test/start', {
			mode: 'identify',
			identify: [{ nodeId: S.ids.f2, output: 1, color: '#00ff00', blinks: 0 }]
		});
		await until(
			'f2 output 1 green, the rest dark',
			async () => {
				const t = await tap(F2);
				return (
					t.rgb[0].length &&
					t.rgb[0].every((v, i) => (i % 3 === 1 ? v === 255 : v === 0)) &&
					t.rgb.slice(1).every((o) => !lit(o))
				);
			},
			{ timeout: 3000 }
		);
		await L.post('/test/stop');
		// Back to the scheduled show for the next phase.
		await L.post('/player/play', { playlistId: S.playlist.id });
	});
}

// Fleet operations (WS5): update status of every controller, update settings, remote access
// status, and the encrypted controller transfer file (F10 / F14 / F15).
async function phaseFleet() {
	await step('updates: every controller listed, settings validated', async () => {
		const u = await L.get('/system/update');
		check(typeof u.current === 'string' && Array.isArray(u.history), 'update info shape');
		eq(u.nodes.length, 3, 'three controllers in the update list');
		check(
			u.nodes.every((n) => typeof n.version === 'string' && typeof n.canApply === 'boolean'),
			'versions of every node'
		);
		const settings = { channel: 'stable', auto: 'notify', window: { from: '09:30', to: '11:00', days: ['sat'] }, avoidShowHours: 3 };
		eq(await L.put('/system/update/settings', settings), settings, 'saved update settings');
		await L.put('/system/update/settings', { ...settings, window: { from: 'soon', to: '11:00', days: [] } }, { expect: 400 });
		eq((await L.get('/show')).settings.updates.window.from, '09:30', 'kept in the show');
		// A follower is updated by its leader.
		await F1.post('/system/update/rollback', {}, { expect: 409 });
	});

	await step('remote access status (no helper in dev: explains itself)', async () => {
		const r = await L.get('/remote/status');
		check(typeof r.tailscale.state === 'string' && Array.isArray(r.cloudflare.urls), 'remote status shape');
		eq(r.canManage, false, 'not manageable without the packaged helper');
		await L.post('/remote/tailscale/install', {}, { expect: 403 });
		await L.post('/remote/test', { url: 'https://example.com/' }, { expect: 400 });
	});

	await step('controller transfer file: one-time link, encrypted stream', async () => {
		await L.post('/system/transfer/export', { passphrase: 'short' }, { expect: 400 });
		const { url } = await L.post('/system/transfer/export', { passphrase: 'e2e-transfer-passphrase' });
		const origin = L.base.replace(/\/api\/v1$/, '');
		const headers = { 'X-PixelPlus-Request': '1', ...(L.cookie ? { cookie: L.cookie } : {}) };
		const r = await fetch(origin + url, { headers });
		eq(r.status, 200, 'download');
		check(/\.ppxfer"$/.test(r.headers.get('content-disposition') ?? ''), 'file name');
		const body = Buffer.from(await r.arrayBuffer());
		eq(body.subarray(0, 6).toString(), 'PPXFER', 'magic');
		check(body.length > 1000, `holds the show and its files (${body.length} bytes)`);
		eq((await fetch(origin + url, { headers })).status, 404, 'the link works once');
		// Followers have nothing to transfer.
		await F2.post('/system/transfer/export', { passphrase: 'e2e-transfer-passphrase' }, { expect: 409 });
	});
}

async function phasePublic() {
	const PUB = `http://127.0.0.1:${Number(process.env.PP_PUBLIC_BASE ?? 18090)}`;
	const games = await fakeGames();
	try {
		await step('public listener: off until enabled, then public pages only', async () => {
			eq((await fetch(PUB + '/api/v1/public/health')).status, 503, 'off by default');
			await L.put('/show/settings', { remote: { publicListener: true }, games: { port: games.port } });
			const h = await fetch(PUB + '/api/v1/public/health');
			eq(h.status, 200, 'public health');
			eq(h.headers.get('connection'), 'close', 'every response closes its connection');
			eq((await fetch(PUB + '/api/v1/show')).status, 404, 'admin API hidden');
			eq((await fetch(PUB + '/settings')).status, 404, 'admin pages hidden');
			const login = await fetch(PUB + '/api/v1/auth/login', {
				method: 'POST',
				headers: { 'content-type': 'application/json', 'X-PixelPlus-Request': '1' },
				body: '{"password":"x"}'
			});
			eq(login.status, 404, 'no sign-in on the public listener');
			const root = await fetch(PUB + '/', { redirect: 'manual' });
			eq([root.status, root.headers.get('location')], [307, '/request'], '/ goes to the request page');
			const page = await fetch(PUB + '/request');
			eq(page.status, 200, 'request page');
			check(/<html/i.test(await page.text()), 'request page is HTML');
			eq((await fetch(PUB + '/api/v1/public/requests')).status, 200, 'public requests API');
		});

		await step('public listener: /play proxied to the games controller (HTTP and WebSocket)', async () => {
			const redirect = await fetch(PUB + '/play', { redirect: 'manual' });
			eq([redirect.status, redirect.headers.get('location')], [308, '/play/'], '/play → /play/');
			const page = await fetch(PUB + '/play/');
			eq(page.status, 200, 'games page through the proxy');
			check(/fake games controller/.test(await page.text()), 'games page body');
			check(
				games.seen.some((x) => x.url === '/' && /127\.0\.0\.1/.test(x.xff)),
				`path rewritten, visitor forwarded: ${JSON.stringify(games.seen)}`
			);
			const ws = new WebSocket(PUB.replace('http', 'ws') + '/play/ws');
			const reply = await new Promise((resolve, reject) => {
				const t = setTimeout(() => reject(new Fail('no WebSocket echo through /play/ws')), 5000);
				ws.onopen = () => ws.send('hello lights');
				ws.onmessage = (e) => {
					clearTimeout(t);
					resolve(String(e.data));
				};
				ws.onerror = () => {
					clearTimeout(t);
					reject(new Fail('WebSocket through /play/ws failed'));
				};
			});
			ws.close();
			eq(reply, 'echo:hello lights', 'WebSocket frames pass both ways');
			check(games.seen.some((x) => x.upgrade && x.url === '/ws'), 'upgrade reached the games controller');
		});

		await step('feature toggles: Games and Song requests off → public pages 404; back on → they work', async () => {
			await L.put('/show/settings', { requests: { enabled: true } });
			const off = await L.put('/features', { disabled: ['games', 'requests'] });
			eq(off.disabled, ['games', 'requests'], 'features saved');
			eq((await L.get('/show')).settings.features.disabled, ['games', 'requests'], 'visible in GET /show');
			const play = await fetch(PUB + '/play/');
			eq(play.status, 404, '/play/ while games are off');
			const pubReq = await fetch(PUB + '/api/v1/public/requests');
			eq(pubReq.status, 404, 'public requests through the tunnel while off');
			eq((await pubReq.json()).error.code, 'feature_disabled', 'feature_disabled code');
			await L.get('/public/requests', { expect: 404 });
			const games = await L.get('/games/status', { expect: 409 });
			eq(games.error.code, 'feature_disabled', 'admin games API');
			check(/Settings → Features/.test(games.error.message), `friendly message: ${games.error.message}`);
			eq((await fetch(PUB + '/api/v1/public/health')).status, 200, 'other public pages keep working');
			const on = await L.put('/features', { id: 'games', enabled: true });
			eq(on.changed, ['games'], 'one switch');
			await L.put('/features', { id: 'requests', enabled: true });
			eq((await fetch(PUB + '/play/')).status, 200, '/play/ back on');
			eq((await fetch(PUB + '/api/v1/public/requests')).status, 200, 'public requests back on');
			await L.get('/games/status');
		});

		await step('visitor caps: hourly song requests per visitor and for everyone (forwarded address)', async () => {
			await L.put('/show/settings', { requests: { enabled: true, maxQueue: 5, perVisitorPerHour: 2, maxPerHour: 60 } });
			const request = async (ip) => {
				const r = await fetch(PUB + '/api/v1/public/requests', {
					method: 'POST',
					headers: { 'content-type': 'application/json', 'X-PixelPlus-Request': '1', 'X-Forwarded-For': ip },
					body: JSON.stringify({ sequenceId: S.seq2.id })
				});
				const body = await r.json();
				for (const q of await L.get('/requests')) await L.del(`/requests/${q.id}`);
				return [r.status, body.error?.code ?? 'ok'];
			};
			eq(await request('198.51.100.7'), [200, 'ok'], 'first request');
			eq(await request('198.51.100.7'), [200, 'ok'], 'second request');
			eq(await request('198.51.100.7'), [429, 'rate_limited'], 'third request from the same visitor');
			eq(await request('198.51.100.8'), [200, 'ok'], 'another visitor');
			await L.put('/show/settings', { requests: { maxPerHour: 1 } });
			eq(await request('198.51.100.9'), [429, 'busy'], 'everyone together: hourly cap');
			await L.put('/show/settings', { requests: { perVisitorPerHour: 6, maxPerHour: 60 } });
		});
	} finally {
		games.close();
		await L.put('/show/settings', { remote: { publicListener: false } }).catch(() => {});
	}
}

async function phaseSensors() {
	const sensorPort = Number(process.env.PP_CLUSTER_BASE ?? 33420) + 2;
	const sim = await new SimSensor({ id: 'sne2e00001', sensorPort }).start();
	try {
		await step('sensor node: discovered, adopted with a key exchange, heartbeats show it live', async () => {
			await until('the sensor is discovered', async () =>
				(await L.get('/sensor-nodes/discovered')).some((d) => d.id === sim.id && d.inputs.includes('pir1'))
			);
			const node = await L.post('/sensor-nodes/adopt', { id: sim.id });
			eq([node.id, node.inputs.map((i) => [i.id, i.kind])], [sim.id, [['pir1', 'motion']]], 'adopted node');
			check(sim.key?.length === 64, 'the sensor derived its key');
			check((await L.get('/sensor-nodes')).some((n) => n.id === sim.id), 'listed');
			const ack = await sim.status();
			check(ack?.ok, `heartbeat acknowledged ${JSON.stringify(ack)}`);
			await until('live and online', async () => {
				const live = (await L.get('/sensor-nodes/live'))[sim.id];
				return live?.online && live.rssi === -58 && live.inputs.pir1 === 0;
			});
		});

		await step('sensor surprise: motion fires the trigger on the rising edge; forged packets are ignored', async () => {
			const red = (await L.get('/effects')).find((e) => e.name === 'E2E surprise red');
			check(red, 'the surprise look from the engine phase');
			const arch = S.props['Big Arch'];
			const archRed = (t) => t.rgb[0].subarray(0, 50 * 3).every((v, i) => (i % 3 === 0 ? v === 255 : v === 0));
			await L.post('/player/play', { sequenceId: S.seq.id });
			await until('song playing', async () => (await L.get('/player')).item?.id === S.seq.id);
			const show = await L.get('/show');
			const action = { type: 'surprise', ref: red.id, source: 'effect', target: { propIds: [arch.id] }, durationMs: 1200 };
			await L.put('/show/settings', {
				triggers: [
					...(show.settings.triggers ?? []),
					{ id: 'e2emotion', name: 'E2E motion', kind: 'sensor', sensor: { sensorNodeId: sim.id, input: 'pir1' }, action }
				]
			});
			const ack = await sim.event('pir1', 1);
			check(ack?.ok, `event acknowledged ${JSON.stringify(ack)}`);
			await until('Big Arch red on f1', async () => archRed(await tap(F1)), { timeout: 5000, every: 30 });
			await until('the surprise ends', async () => !archRed(await tap(F1)), { timeout: 5000 });
			// Release (falling edge): nothing fires.
			check((await sim.event('pir1', 0))?.ok, 'release acknowledged');
			await sleep(800);
			check(!archRed(await tap(F1)), 'a release fires nothing');
			// A forged packet (wrong key) is dropped and counted.
			const forged = await sim.event('pir1', 1, { key: 'ab'.repeat(32) });
			eq(forged, null, 'no ack for a forged event');
			await sleep(800);
			check(!archRed(await tap(F1)), 'a forged event fires nothing');
			const live = (await L.get('/sensor-nodes/live'))[sim.id];
			check(live.events >= 1 && live.rejected >= 1, `live counters ${JSON.stringify(live)}`);
			// The "Test" button runs the action without the trigger gates.
			const t = await L.post('/surprises/test', { action });
			check(t.ok !== false, `surprise test ${JSON.stringify(t)}`);
			await until('Big Arch red again (test)', async () => archRed(await tap(F1)), { timeout: 5000, every: 30 });
			await L.put('/show/settings', { triggers: show.settings.triggers ?? [] });
			await L.post('/player/stop');
		});
	} finally {
		sim.stop();
	}
}

async function phaseLinks() {
	const PUB = `http://127.0.0.1:${Number(process.env.PP_PUBLIC_BASE ?? 18090)}`;
	const curl = (args) => {
		const r = spawnSync('curl', ['-s', '-o', '/dev/null', '-w', '%{http_code}', ...args], { encoding: 'utf8' });
		if (r.error) throw new Fail(`curl: ${r.error.message}`);
		return Number(r.stdout);
	};
	await step('trigger link: with a password set, curl fires a surprise with the token alone; revoke → 401', async () => {
		const red = (await L.get('/effects')).find((e) => e.name === 'E2E surprise red');
		check(red, 'the surprise look from the engine phase');
		const arch = S.props['Big Arch'];
		const archRed = (t) => t.rgb[0].subarray(0, 50 * 3).every((v, i) => (i % 3 === 0 ? v === 255 : v === 0));
		const show = await L.get('/show');
		const before = show.settings.triggers ?? [];
		const action = { type: 'surprise', ref: red.id, source: 'effect', target: { propIds: [arch.id] }, durationMs: 1200 };
		await L.put('/show/settings', {
			triggers: [...before, { id: 'e2edoorbell', name: 'E2E doorbell', kind: 'http', action }]
		});
		await L.put('/auth/password', { password: 'e2e-secret' });
		await L.post('/auth/login', { password: 'e2e-secret' });
		try {
			const made = await L.post('/triggers/e2edoorbell/token');
			check(/^ppt_[\w-]{43}$/.test(made.token), `token ${made.token}`);
			const url = `${L.base}/hooks/trigger/e2edoorbell`;
			// The secret never comes back from GET /show.
			const shown = JSON.stringify(await L.get('/show'));
			check(!shown.includes(made.token) && !shown.includes('tokenHash'), 'no secrets in GET /show');
			check(shown.includes(`"tokenHint":"${made.token.slice(-4)}"`), 'hint in GET /show');
			// No session, no CSRF header: just the token (curl, like Home Assistant's rest_command).
			eq(curl(['-X', 'POST', url]), 401, 'no token');
			eq(curl(['-X', 'POST', '-H', 'Authorization: Bearer ppt_wrong', url]), 401, 'wrong token');
			eq(curl(['-X', 'POST', '-H', `Authorization: Bearer ${made.token}`, url]), 202, 'fired with the token');
			await until('Big Arch red on f1 (link)', async () => archRed(await tap(F1)), { timeout: 5000, every: 30 });
			await until('the surprise ends', async () => !archRed(await tap(F1)), { timeout: 5000 });
			eq(curl([`${url}?token=${made.token}`]), 405, 'GET is off by default');
			eq(curl(['-X', 'POST', `${url}?token=${made.token}`]), 202, 'token in the query');
			const uses = await L.get('/triggers/links');
			eq([uses.links.e2edoorbell?.fired, uses.links.e2edoorbell?.origin], [true, 'home'], 'last use recorded');
			// Through the public listener (tunnels): invisible until allowed from the internet.
			await L.put('/show/settings', { remote: { publicListener: true } });
			const pub = (tok) =>
				fetch(`${PUB}/api/v1/hooks/trigger/e2edoorbell`, { method: 'POST', headers: { authorization: `Bearer ${tok}` } });
			eq((await pub(made.token)).status, 404, 'home-only link through the tunnel');
			const t = (await L.get('/show')).settings.triggers.find((x) => x.id === 'e2edoorbell');
			await L.put('/show/settings', {
				triggers: [...before, { ...t, allowInternet: true }]
			});
			await sleep(1300);
			eq((await pub(made.token)).status, 202, 'allowed from the internet');
			await L.put('/show/settings', { remote: { publicListener: false } });
			// Rotate: the old token stops working; revoke: no token works.
			const again = await L.post('/triggers/e2edoorbell/token');
			eq(curl(['-X', 'POST', '-H', `Authorization: Bearer ${made.token}`, url]), 401, 'old token after rotate');
			await sleep(1300);
			eq(curl(['-X', 'POST', '-H', `Authorization: Bearer ${again.token}`, url]), 202, 'new token');
			await L.del('/triggers/e2edoorbell/token');
			eq(curl(['-X', 'POST', '-H', `Authorization: Bearer ${again.token}`, url]), 401, 'revoked');
		} finally {
			await L.put('/show/settings', { triggers: before, remote: { publicListener: false } }).catch(() => {});
			await L.put('/auth/password', { current: 'e2e-secret', password: null }).catch(() => {});
			L.cookie = '';
		}
	});
}

// ---------------------------------------------------------------------------

const PHASES = [
	['setup', phaseSetup],
	['import', phaseImport],
	['content', phaseContent],
	['show', phaseShow],
	['tools', phaseTools],
	['engine', phaseEngine],
	['fleet', phaseFleet],
	['public', phasePublic],
	['resilience', phaseResilience],
	['sensors', phaseSensors],
	['links', phaseLinks]
];

console.log(`PixelPlus e2e — cluster in ${DIR}`);
try {
	for (const [name, fn] of PHASES) {
		console.log(`\n# ${name}`);
		await fn();
		if (results.some((r) => !r.ok) && ['setup', 'import', 'content'].includes(name)) {
			console.log('(stopping: later phases depend on this one)');
			break;
		}
		if (ONLY && ONLY.at(-1) === name) break;
	}
} finally {
	if (!KEEP) {
		try {
			cluster('stop');
		} catch {
			/* already down */
		}
	} else {
		console.log(`\nCluster left running (scripts/dev-cluster.sh stop with PP_CLUSTER_DIR=${DIR}).`);
		console.log(`Leader: http://127.0.0.1:${URL_OF.leader}`);
	}
}
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} steps passed`);
for (const f of failed) console.log(`  ✗ ${f.name}: ${f.error}`);
if (!KEEP && !failed.length && OWN_DIR) fs.rmSync(DIR, { recursive: true, force: true });
process.exit(failed.length ? 1 : 0);

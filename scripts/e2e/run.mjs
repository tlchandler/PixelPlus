#!/usr/bin/env node
// End-to-end scenario against a REAL three-node cluster (leader + two followers on
// this machine, started with scripts/dev-cluster.sh): setup wizard, discovery and
// adoption, xLights import, .fseq + audio upload, follower slices (byte-checked),
// playlist + schedule, frame-accurate sync across nodes (via GET /debug/output),
// tools (tests, fault finder, blackout, brightness, looks, overlays, requests,
// snapshots, health, power, sensors, games/TTS without sidecars), password,
// follower restart mid-show, leader crash, live prop changes, remove + re-adopt.
//
//   cargo build -p pixelplus-daemon && (cd web && pnpm build)
//   node scripts/e2e/run.mjs            # fresh cluster in a temp dir, stopped afterwards
//   node scripts/e2e/run.mjs --keep     # leave the cluster running (e.g. for the Playwright
//                                       # suite: cd web && pnpm test:real)
//   node scripts/e2e/run.mjs --only=sync,tools   # stop after the named phases
//
// Environment: PP_BIN, PP_WEB_DIR, PP_HTTP_BASE, PP_CLUSTER_BASE, PP_CLUSTER_DIR
// (see scripts/dev-cluster.sh). Needs Node >= 22.15 (zstd, WebSocket).
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
	channelCount,
	fillFrame,
	MARKER_B,
	nodeFrame,
	readPpseq,
	writeFseq,
	writeWav
} from './media.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const args = new Set(process.argv.slice(2));
const KEEP = args.has('--keep');
const ONLY = [...args].find((a) => a.startsWith('--only='))?.slice(7).split(',');
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
	if (JSON.stringify(a) !== JSON.stringify(b)) throw new Fail(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`);
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
	throw new Fail(`timed out after ${timeout} ms waiting for ${what}${last instanceof Error ? ` (${last.message})` : ''}`);
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
	async function call(method, p, body, { expect = 200, raw = false } = {}) {
		// Like the web UI: mark API calls as coming from our own client (CSRF guard).
		const headers = { 'X-PixelPlus-Request': '1' };
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
			if (expect !== null && r.status !== expect) throw new Fail(`${method} ${node}${p} → ${r.status}, expected ${expect}`);
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
		rgb: d.outputs.map((o) => Buffer.from(o.rgb, 'base64')),
		wire: d.outputs.map((o) => Buffer.from(o.wire, 'base64')),
		ppo: d.outputs.map((o) => o.pixels)
	};
}
const lit = (buf) => buf.some((b) => b !== 0);

// ---------------------------------------------------------------------------
// Scenario state
// ---------------------------------------------------------------------------

const S = { ids: {}, props: {}, show: null, seq: null, seq2: null, media: null, playlist: null, fseqBytes: null };
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
		for (const c of Object.values(NODES)) eq((await c.get('/public/health')).role, 'unconfigured', 'fresh role');
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
		eq(show.nodes.map((n) => [n.id, n.role, n.outputs.length]), [[ls.nodeId, 'leader', 60]], 'leader node in show');
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
		eq([sys.role, sys.leaderName, sys.showName], ['follower', 'Main', 'E2E Show'], 'follower knows its leader');
		await until('f1 manifest (show name)', async () => (await F1.get('/show')).name === 'E2E Show');
	});
}

async function phaseImport() {
	await step('xLights import: preview, match controllers, apply', async () => {
		const td = path.join(ROOT, 'crates/pixelplus-core/testdata');
		const fd = new FormData();
		fd.append('rgbeffects', new Blob([fs.readFileSync(path.join(td, 'xlights_2025_rgbeffects.xml'))]), 'xlights_rgbeffects.xml');
		fd.append('networks', new Blob([fs.readFileSync(path.join(td, 'xlights_2025_networks.xml'))]), 'xlights_networks.xml');
		const preview = await L.post('/import/xlights', fd);
		eq(preview.controllers.map((c) => c.name).sort(), ['Front PixelPlus', 'Tree F48'], 'controllers');
		check(preview.props.length >= 9, `props in preview: ${preview.props.length}`);
		check(preview.warnings.some((w) => /Window/.test(w)), 'unassigned model warned');
		const show = await L.post('/import/xlights/apply', {
			preview,
			controllerMap: { 'Front PixelPlus': S.ids.f1, 'Tree F48': S.ids.leader }
		});
		for (const p of show.props) S.props[p.name] = p;
		// "Front PixelPlus" has 5 ports in xLights, the pHAT only 4: port 5 is left unwired.
		const poly = S.props['Garage Poly'];
		eq(poly.segments.map((s) => [s.nodeId, s.output]), [[S.ids.f1, 4]], 'Garage Poly wiring after import');
		const h = await L.post('/health/run');
		const wiring = h.checks.find((c) => c.id === 'wiring');
		check(wiring.status === 'warn' && /Partly wired: Garage Poly/.test(wiring.detail), `wiring check: ${wiring.detail}`);
		check(/Window not wired/.test(wiring.detail), `unwired Window: ${wiring.detail}`);
	});

	await step('wire the rest by hand (Garage Poly 2nd half + Window on f2)', async () => {
		const poly = structuredClone(S.props['Garage Poly']);
		poly.segments.push({ nodeId: S.ids.f2, output: 1, startPixel: 0, pixelCount: 75, propOffset: 75, reverse: true, nullPixels: 0 });
		// A segment on an output the board doesn't have is refused with a clear message.
		const bad = structuredClone(poly);
		bad.segments[1].output = 5;
		const err = await L.put(`/props/${poly.id}`, bad, { expect: 400 });
		check(/Garage doesn't have output 5/.test(err.error.message), err.error.message);
		await L.put(`/props/${poly.id}`, poly);
		const win = structuredClone(S.props['Window']);
		win.segments = [{ nodeId: S.ids.f2, output: 2, startPixel: 2, pixelCount: 60, propOffset: 0, reverse: false, nullPixels: 2 }];
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
		const wav = writeWav({ seconds: SONG_FRAMES * FRAME_MS / 1000 });
		const mf = new FormData();
		mf.append('file', new Blob([wav]), 'E2E Song.wav');
		S.media = await L.post('/media', mf);
		eq([S.media.kind, S.media.durationMs], ['song', 20000], 'media');
		check(typeof S.media.loudnessLufs === 'number', 'loudness measured');
		const sf = new FormData();
		sf.append('fseq', new Blob([S.fseqBytes]), 'E2E Song.fseq');
		S.seq = await L.post('/sequences', sf);
		eq([S.seq.durationMs, S.seq.frameMs, S.seq.channelCount, S.seq.mediaId], [20000, 50, S.channels, S.media.id], 'sequence');
		const th = await L.raw('GET', `/sequences/${S.seq.id}/thumbnail`);
		eq(th.headers.get('content-type'), 'image/png', 'thumbnail type');
		const png = Buffer.from(await th.arrayBuffer());
		eq(png.subarray(1, 4).toString(), 'PNG', 'thumbnail is a PNG');
		const peaks = await L.get(`/media/${S.media.id}/peaks?n=50`);
		check(peaks.length === 50 && peaks.every((p) => p >= 0 && p <= 1) && Math.max(...peaks) > 0.1, 'waveform peaks');
	});

	await step('upload a second sequence together with its audio', async () => {
		const fd = new FormData();
		const short = writeFseq({ channelCount: S.channels, frameMs: 25, frames: 200, frame: (i, b) => fillFrame(S.show.props, i, 25, b) });
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
				if (!exp.equals(s.frame(i))) throw new Fail(`${f}: slice frame ${i} differs from the leader's rendering`);
			}
			log(`${f}: ${s.frameCount} frames, outputs ${s.ppo.join('/')} px — all bytes match`);
		}
		check(!fs.existsSync(path.join(DIR, 'leader', 'sequences', `${S.seq.id}.ppseq`)), 'leader plays the fseq itself');
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
		const pl = (await L.get('/playlists'))[0] ?? (await L.post('/playlists', { name: 'Main Show', items: [] }));
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
		check(prev.some((o) => o.entryId === 'e2e-now'), 'schedule preview lists the entry');
	});

	await step('scheduler starts the show; WebSocket status progresses', async () => {
		const st = await until('player playing the sequence', async () => {
			const p = await L.get('/player');
			return p.state === 'playing' && p.item?.id === S.seq.id && p;
		}, { timeout: 20000 });
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
		check(msgs.some((m) => m.type === 'nodes'), 'nodes message on connect');
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
			for (const [name, t] of [['f1', a], ['f2', b]]) {
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

	await step('seek, pause/resume keep the followers in step', async () => {
		await L.post('/player/seek', { posMs: 12000 });
		await sleep(700);
		const [l, a] = await Promise.all([tap(L), tap(F1)]);
		check(l.frame >= 240 && l.frame < 260, `leader frame after seek ${l.frame}`);
		check(Math.abs(a.frame - l.frame - (a.wallMs - l.wallMs) / FRAME_MS) <= 1.5, `f1 after seek ${a.frame} vs ${l.frame}`);
		await L.post('/player/pause');
		await sleep(600);
		const p1 = await Promise.all([tap(L), tap(F2)]);
		await sleep(600);
		const p2 = await Promise.all([tap(L), tap(F2)]);
		eq([p2[0].frame, p2[1].frame], [p1[0].frame, p1[1].frame], 'paused frames hold');
		check(Math.abs(p2[0].frame - p2[1].frame) <= 1, `paused on the same frame ${p2[0].frame}/${p2[1].frame}`);
		eq((await F2.get('/player')).state, 'paused', 'follower paused');
		await L.post('/player/resume');
		await until('playing again', async () => (await F2.get('/player')).state === 'playing', { timeout: 3000 });
	});

	await step('playlist moves on: DJ clip → pause → look (followers run the look)', async () => {
		await L.post('/player/seek', { posMs: 19000 });
		await until('DJ clip', async () => (await L.get('/player')).item?.type === 'dj', { timeout: 8000 });
		await until('pause', async () => (await L.get('/player')).item?.type === 'pause', { timeout: 8000 });
		await until('look', async () => (await L.get('/player')).item?.type === 'effect', { timeout: 8000 });
		await sleep(400);
		const [l, a, b] = await Promise.all([tap(L), tap(F1), tap(F2)]);
		check(lit(Buffer.concat(l.rgb)) && lit(Buffer.concat(a.rgb)) && lit(Buffer.concat(b.rgb)), 'look lights every node');
		eq((await F1.get('/player')).state, (await L.get('/player')).state, 'follower state follows the leader');
		await until('back to the sequence (repeat)', async () => (await L.get('/player')).item?.id === S.seq.id, { timeout: 8000 });
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
			for (let i = 0; i < px.length; i += 3) if (px[i] || px[i + 1] || px[i + 2]) pixels.push(px.subarray(i, i + 3).toString('hex'));
			check(pixels.length > 0, `${n}: test pattern lit nothing`);
			check(pixels.every((p) => p === 'ff0000'), `${n}: not solid red (${[...new Set(pixels)].slice(0, 4)})`);
		}
		eq((await L.get('/player')).state, 'testing', 'leader state testing');
		// Raw output test on one follower port.
		await L.post('/test/start', { mode: 'solid', color: '#00ff00', target: { nodeId: S.ids.f2, output: 2 } });
		await sleep(600);
		const t = await tap(F2);
		check(t.rgb[1].length && t.rgb[1].every((v, i) => (i % 3 === 1 ? v === 255 : v === 0)), 'f2 output 2 all green');
		await L.post('/test/stop');
		await until('show back after the test', async () => (await L.get('/player')).state !== 'testing', { timeout: 3000 });
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
		check(lit(out1.subarray(0, 50 * 3)) && !lit(out1.subarray(50 * 3)), 'f1 lights exactly pixels 1–50 of Big Arch');
		// Pretend pixel 38 (index 37) is broken: "yes" while all lit pixels are before it.
		let guard = 0;
		while (!st.done && guard++ < 12) st = await L.post(`/faultfinder/${st.session}/answer`, { lit: st.litTo <= 37 });
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
		check(lit(arch) && [...arch].every((v, i) => (i % 3 === 2 ? v > 0 : v === 0)), 'Big Arch solid blue on f1');
		check(!lit(t.rgb[1]), 'Canes (not targeted) dark');
		await L.post('/player/effect', { effect: null });
	});

	await step('overlays: text on the matrix, friendly QR/size errors', async () => {
		const m = S.props['Matrix'];
		check(m.matrix?.width > 0, 'Matrix has matrix geometry');
		await L.post(`/overlay/${m.id}/text`, { text: 'HELLO', color: '#ff0000', durationMs: 2000 });
		const qr = await L.post(`/overlay/${m.id}/qr`, { url: 'http://pixelplus.local/request', durationMs: 2000 }, { expect: null });
		check(qr.ok || /QR code/.test(qr.error?.message ?? ''), `QR answer: ${JSON.stringify(qr)}`);
		const nm = await L.post(`/overlay/${S.props['Big Arch'].id}/text`, { text: 'x' }, { expect: 400 });
		check(/matrix/.test(nm.error.message), nm.error.message);
		await L.post(`/overlay/${m.id}`, { enabled: false });
	});

	await step('song requests: public page, submit, admin queue', async () => {
		await L.put('/show/settings', { requests: { enabled: true, maxQueue: 5, title: 'Pick a song', message: 'Tune to 88.1' } });
		const pub = await L.get('/public/requests');
		eq([pub.enabled, pub.title], [true, 'Pick a song'], 'public request page');
		check(pub.songs.some((s) => s.sequenceId === S.seq.id), 'songs listed');
		const r = await L.post('/public/requests', { sequenceId: S.seq2.id, name: 'Ana' });
		check(r.ok && r.position >= 1, 'request accepted');
		const q = await L.get('/requests');
		check(q.some((x) => x.sequenceId === S.seq2.id), 'admin queue has it');
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
		check(list.some((s) => s.id === snap.id), 'listed');
		await L.del(`/snapshots/${snap.id}`);
		await until('followers back on the restored show', async () => (await F1.get('/show')).name === 'E2E Show');
	});

	await step('health, power estimate, sensors', async () => {
		await until('followers synced', async () => (await L.get('/nodes')).every((n) => n.syncState === 'synced'));
		const h = await L.post('/health/run');
		const ids = h.checks.map((c) => c.id);
		for (const id of ['followers', 'disk', 'audio', 'output', 'clock', 'sequences', 'wiring', 'schedule']) check(ids.includes(id), `health check ${id}`);
		const audio = h.checks.find((c) => c.id === 'audio');
		check(audio.status === 'warn' && /turned off/.test(audio.detail), `audio check with PIXELPLUS_AUDIO=none: ${audio.detail}`);
		eq(h.checks.find((c) => c.id === 'followers').status, 'ok', 'controllers ok');
		const p = await L.get(`/power/estimate?sequenceId=${S.seq.id}`);
		check(p.perOutput.length > 20 && p.perProp.length === S.show.props.length, 'power per output / prop');
		check(p.perOutput.every((o) => o.peakAmps >= o.avgAmps), 'peak ≥ average');
		const sensors = await until('simulated sensors (PIXELPLUS_DEV)', async () => {
			const s = await L.get('/system/sensors');
			return s.some((x) => x.id === 'inputVoltage') && s;
		}, { timeout: 12000 });
		check(sensors.every((s) => typeof s.value === 'number' && s.unit && s.nodeId), 'sensor shape');
		const hist = await L.get('/system/sensors/history?minutes=10');
		check(Object.keys(hist.series).length > 0, 'sensor history');
	});

	await step('games and TTS without their sidecars degrade gracefully', async () => {
		const g = await L.get('/games/status');
		eq([g.running, g.available], [false, false], 'games not running');
		const inv = await L.post('/games/invite', {}, { expect: null });
		check(inv?.error?.message && !/panic|internal/i.test(inv.error.message), `games invite: ${JSON.stringify(inv)}`);
		const t = await L.get('/tts/status');
		eq([t.mode, t.available], ['browser', true], 'TTS falls back to the browser');
	});

	await step('password: set, 401 without session, login/logout, cluster keeps working', async () => {
		await L.put('/auth/password', { password: 'e2e-secret' });
		await L.get('/show', { expect: 401 });
		eq((await L.get('/public/health')).ok, true, 'public health stays open');
		await L.get('/public/requests');
		const sys = await L.get('/system');
		check(sys.passwordSet === true && sys.ips === undefined && sys.cpuPct === undefined, 'unauthenticated /system is minimal');
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
		await L.post('/auth/login', { password: 'e2e-secret' });
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
			Math.abs(wire[0] - half(rgb[1])) <= 1 && Math.abs(wire[1] - half(rgb[0])) <= 1 && Math.abs(wire[2] - half(rgb[2])) <= 1,
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
		await until('followers online again', async () => (await L.get('/nodes')).every((n) => n.online), { timeout: 15000 });
		// The schedule is still active: the leader restarts the show by itself.
		await until('show running again', async () => (await L.get('/player')).state === 'playing', { timeout: 20000 });
		await until('f2 in sync again', () => inSync(F2, 'f2'), { timeout: 15000 });
	});

	await step('remove a follower, re-adopt it', async () => {
		const res = await L.del(`/nodes/${S.ids.f2}?force=1`);
		check(res.ok !== false, 'deleted');
		await until('f2 released', async () => (await F2.get('/system')).leaderName == null, { timeout: 8000 });
		const show = await L.get('/show');
		check(!show.nodes.some((n) => n.id === S.ids.f2), 'f2 gone from the show');
		check(show.props.every((p) => p.segments.every((s) => s.nodeId !== S.ids.f2)), 'its wiring is gone');
		await until('f2 discovered again', async () => (await L.get('/nodes/discovered')).some((n) => n.id === S.ids.f2), { timeout: 10000 });
		await L.post('/nodes/adopt', { id: S.ids.f2, name: 'Garage' });
		await until('f2 online', async () => (await L.get('/nodes')).some((n) => n.id === S.ids.f2 && n.online && n.adopted));
		eq((await F2.get('/system')).leaderName, 'Main', 'f2 follows Main again');
		// Wire it again so later runs (and the UI walk) see a complete show.
		const cur = await L.get('/show');
		const poly = cur.props.find((p) => p.name === 'Garage Poly');
		poly.segments.push({ nodeId: S.ids.f2, output: 1, startPixel: 0, pixelCount: 75, propOffset: 75, reverse: true, nullPixels: 0 });
		await L.put(`/props/${poly.id}`, poly);
		const win = cur.props.find((p) => p.name === 'Window');
		win.segments = [{ nodeId: S.ids.f2, output: 2, startPixel: 2, pixelCount: 60, propOffset: 0, reverse: false, nullPixels: 2 }];
		await L.put(`/props/${win.id}`, win);
		await until('f2 slices again', async () => {
			const n = (await L.get('/nodes')).find((x) => x.id === S.ids.f2);
			return n.files.total === 2 && n.files.pending === 0;
		}, { timeout: 20000 });
	});
}

// ---------------------------------------------------------------------------

const PHASES = [
	['setup', phaseSetup],
	['import', phaseImport],
	['content', phaseContent],
	['show', phaseShow],
	['tools', phaseTools],
	['resilience', phaseResilience]
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

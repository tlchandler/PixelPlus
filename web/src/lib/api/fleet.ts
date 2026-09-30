// WS5 fleet operations: controller replacement and transfer (F10), remote access (F14),
// signed cluster updates (F15). Thin typed wrappers over `request` (see ARCHITECTURE §12.9,
// §12.12, §12.13); the mock backend serves the same paths (lib/mock/feat/{remote,updates}.ts).
import { request } from './client';
import type * as T from './types';

const get = <R>(p: string) => request<R>('GET', p);
const post = <R>(p: string, b?: unknown) => request<R>('POST', p, b ?? {});
const put = <R>(p: string, b?: unknown) => request<R>('PUT', p, b ?? {});

export type HelperReply = { ok: boolean; job?: T.HelperStatus | null };

export const fleetApi = {
	// F10
	replace: (nodeId: string, candidateId: string, force = false) =>
		post<T.Node>(`/nodes/${encodeURIComponent(nodeId)}/replace`, { candidateId, force }),
	releaseRetired: (nodeId: string) =>
		post<{ ok: boolean }>(`/nodes/${encodeURIComponent(nodeId)}/release-retired`),
	/** A one-time link (10 minutes) that downloads the encrypted transfer file. */
	transferExport: (passphrase: string) =>
		post<{ url: string; expiresInS: number }>('/system/transfer/export', { passphrase }),

	// F14
	remoteStatus: (fresh = false) => get<T.RemoteStatus>(`/remote/status${fresh ? '?fresh=true' : ''}`),
	tailscale: (
		action: 'install' | 'up' | 'serve' | 'funnel' | 'down',
		body?: { on?: boolean; authKey?: string }
	) => post<HelperReply>(`/remote/tailscale/${action}`, body),
	cloudflare: (
		action: 'install' | 'quick' | 'token' | 'hosts' | 'stop',
		body?: { on?: boolean; token?: string; publicHost?: string; adminHost?: string }
	) => post<HelperReply>(`/remote/cloudflare/${action}`, body),
	remoteTest: (url: string) =>
		post<{ ok: boolean; url: string; status?: number; ms: number; error?: string }>('/remote/test', { url }),

	// F15
	updates: (refresh = false) => get<T.UpdateInfo>(`/system/update${refresh ? '?refresh=true' : ''}`),
	startUpdate: (body: { version?: string; scope?: 'cluster' | 'this'; force?: boolean } = {}) =>
		post<{ ok: boolean; message: string; run?: T.UpdateRun; job?: T.HelperStatus | null }>(
			'/system/update',
			body
		),
	rollback: (scope: 'cluster' | 'this' = 'cluster') =>
		post<{ ok: boolean; run: T.UpdateRun }>('/system/update/rollback', { scope }),
	updateSettings: (s: T.UpdateSettings) => put<T.UpdateSettings>('/system/update/settings', s)
};

/** Update phases in words. */
export const runPhaseLabel: Record<T.UpdateRun['phase'], string> = {
	staging: 'Preparing every controller',
	committingFollowers: 'Installing on the followers',
	committingLeader: 'Installing on the show leader',
	rollingBack: 'Putting every controller back',
	done: 'Done',
	rolledBack: 'Rolled back',
	failed: 'Failed'
};

export const nodePhaseLabel: Record<T.UpdateRun['nodes'][number]['phase'], string> = {
	pending: 'Waiting',
	staging: 'Preparing',
	staged: 'Ready',
	committing: 'Installing',
	healthy: 'Updated',
	failed: 'Failed',
	rollingBack: 'Going back',
	rolledBack: 'Back on the old version'
};

/** Passphrase rule shared with the daemon (services/transfer.rs MIN_PASSPHRASE). */
export const MIN_PASSPHRASE = 10;

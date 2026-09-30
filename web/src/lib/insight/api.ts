// WS6 (F8 seasons, F11 reports, F16 xLights, F20 sensor nodes): typed calls to the
// daemon endpoints this workstream serves. Kept here (not in the shared client) so
// the contract files stay frozen; the base types come from `$lib/api/types`.
import { request } from '$lib/api/client';
import type {
	DiscoveredSensorNode,
	NightReport,
	ProfileSwitchDiff,
	ReportSummary,
	SensorNode,
	Show,
	ShowProfile,
	TriggerAction
} from '$lib/api/types';

const get = <R>(p: string) => request<R>('GET', p);
const post = <R>(p: string, b?: unknown) => request<R>('POST', p, b ?? {});
const put = <R>(p: string, b?: unknown) => request<R>('PUT', p, b ?? {});
const del = <R>(p: string) => request<R>('DELETE', p);

// ------------------------------------------------------------------ F11 reports
export type { ReportSeries } from '$lib/api/types';
/** Kept as names for the reports UI; the full shapes live in `$lib/api/types`. */
export type NightReportFull = NightReport;
export type ReportSummaryFull = ReportSummary;

export const reportsApi = {
	list: (limit = 30) => get<ReportSummaryFull[]>(`/reports?limit=${limit}`),
	get: (date: string) => get<NightReportFull>(`/reports/${date}`),
	run: (date?: string, send = false) => post<NightReportFull>('/reports/run', { date, send }),
	emailUrl: (date: string) => `/api/v1/reports/${date}/email`
};

// ------------------------------------------------------------------ F8 seasons
export interface ActiveSeason {
	id: string | null;
	name: string | null;
	icon: string | null;
	color: string | null;
	autoSwitch: boolean;
	scheduledId: string | null;
	nextSwitch: { profileId: string; name: string; date: string } | null;
}

export const profilesApi = {
	list: () => get<ShowProfile[]>('/profiles'),
	create: (p: Partial<ShowProfile>) => post<ShowProfile>('/profiles', p),
	capture: (name: string) => post<ShowProfile>('/profiles/capture', { name }),
	update: (id: string, p: Partial<ShowProfile>) => put<ShowProfile>(`/profiles/${id}`, p),
	remove: (id: string) => del<void>(`/profiles/${id}`),
	activate: (id: string, saveCurrent = true) => post<Show>(`/profiles/${id}/activate`, { saveCurrent }),
	preview: (id: string) => get<ProfileSwitchDiff>(`/profiles/preview-switch/${id}`),
	active: () => get<ActiveSeason>('/profiles/active'),
	autoSwitch: (enabled: boolean) => put<{ enabled: boolean }>('/profiles/auto-switch', { enabled })
};

// ------------------------------------------------------------------ F16 xLights
export interface UploadEntry {
	at: string;
	name: string;
	kind: 'sequence' | 'song' | 'ignored';
	ok: boolean;
	message: string;
	bytes: number;
	source: 'xlights' | 'folder';
	sequenceId?: string;
	mediaId?: string;
	replaced: boolean;
}
export interface XlightsStatus {
	enabled: boolean;
	passwordSet: boolean;
	adminPasswordSet: boolean;
	ready: boolean;
	reason?: string;
	addresses: string[];
	hostname: string;
	uploads: UploadEntry[];
	watch: { folder: string | null; exists: boolean; suggested: string; lastScan?: string; error?: string };
}

export const xlightsApi = {
	status: () => get<XlightsStatus>('/xlights/status'),
	setPassword: (password: string) => put<{ passwordSet: boolean }>('/xlights/password', { password }),
	clearLog: () => del<{ ok: boolean }>('/xlights/uploads')
};

// ------------------------------------------------------------------ F20 sensors
export interface SensorLive {
	online: boolean;
	rssi?: number | null;
	uptimeS?: number | null;
	ip?: string | null;
	ver?: string | null;
	lastSeen?: string | null;
	inputs: Record<string, number>;
	amps: Record<string, number>;
	volts: Record<string, number>;
	events: number;
	rejected: number;
}

export const sensorsApi = {
	list: () => get<SensorNode[]>('/sensor-nodes'),
	discovered: () => get<DiscoveredSensorNode[]>('/sensor-nodes/discovered'),
	adopt: (id: string) => post<SensorNode>('/sensor-nodes/adopt', { id }),
	update: (id: string, n: Partial<SensorNode>) => put<SensorNode>(`/sensor-nodes/${id}`, n),
	release: (id: string) => post<{ ok: boolean; message?: string }>(`/sensor-nodes/${id}/release`),
	identify: (id: string) => post<{ ok: boolean }>(`/sensor-nodes/${id}/identify`),
	live: () => get<Record<string, SensorLive>>('/sensor-nodes/live'),
	testSurprise: (action: TriggerAction) =>
		post<{ ok: boolean; message: string }>('/surprises/test', { action }),
	fireTrigger: (id: string) => post<unknown>(`/triggers/${id}/fire`)
};

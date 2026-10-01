/**
 * Feature-wave GET endpoints the mock already serves (mock/feat/*). Each workstream moves
 * its paths into ENDPOINTS once the daemon implements them, so the contract is checked.
 */
export const PENDING_ENDPOINTS: string[] = [];

/** Feature-wave GET endpoints the daemon serves (checked by contract.test.ts). */
export const ENDPOINTS: string[] = [
	'/tls/status', // WS1
	'/public/tls', // WS1
	'/autoshow/styles', // WS2
	'/library/history?days=14', // WS2
	'/library/tags', // WS2
	'/jobs', // WS2
	'/profiles', // WS6
	'/profiles/active', // WS6
	'/reports?limit=30', // WS6
	'/sensor-nodes', // WS6
	'/sensor-nodes/discovered', // WS6
	'/sensor-nodes/live', // WS6 (a map by node id: see MAPS in contract.test.ts)
	'/xlights/status', // WS6
	'/remote/status', // WS5
	'/power-supplies', // WS3
	'/power/live', // WS3
	'/power/budget', // WS3
	'/mapping/runs', // WS4
	'/triggers/links' // trigger links (§12.18): addresses + last use by trigger id (MAPS)
];

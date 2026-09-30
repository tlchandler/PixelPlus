/**
 * Feature-wave GET endpoints the mock already serves (mock/feat/*). Each workstream moves
 * its paths into ENDPOINTS once the daemon implements them, so the contract is checked.
 */
export const PENDING_ENDPOINTS = [
	'/tls/status', // WS1
	'/autoshow/styles', // WS2
	'/library/history?days=14', // WS2
	'/mapping/runs', // WS4
	'/profiles', // WS6
	'/reports?limit=30', // WS6
	'/remote/status', // WS5
	'/power-supplies', // WS3
	'/power/live', // WS3
	'/sensor-nodes', // WS6
	'/sensor-nodes/discovered' // WS6
];

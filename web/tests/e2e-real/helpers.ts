import type { Page } from '@playwright/test';

/** API answers that are expected on a development machine (no root helper, no Wi-Fi, no games sidecar). */
const EXPECTED_FAILURES: RegExp[] = [/\/system\/network\/scan/, /\/games\/(invite|stop|test)/];

export function watch(page: Page) {
	const problems: string[] = [];
	page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error' && !/Failed to load resource/.test(m.text()))
			problems.push(`console: ${m.text()}`);
	});
	page.on('response', (r) => {
		const url = r.url();
		if (r.status() >= 400 && !EXPECTED_FAILURES.some((re) => re.test(url)))
			problems.push(`${r.request().method()} ${url.replace(/^https?:\/\/[^/]+/, '')} → ${r.status()}`);
	});
	page.on('requestfailed', (r) => {
		const f = r.failure()?.errorText ?? '';
		// Navigations abort in-flight requests; that's not a failure.
		if (!/ERR_ABORTED|NS_BINDING_ABORTED/.test(f)) problems.push(`failed: ${r.url()} ${f}`);
	});
	return problems;
}

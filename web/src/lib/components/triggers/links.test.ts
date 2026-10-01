import { describe, expect, it } from 'vitest';
import {
	curlCommand,
	fallbackAddress,
	homeAssistantAction,
	homeAssistantYaml,
	hookUrl,
	slug,
	usableAddresses,
	withToken
} from './links';

describe('trigger links', () => {
	it('builds the link and the all-in-one URL', () => {
		const url = hookUrl('http://pp.local/', 'tr01');
		expect(url).toBe('http://pp.local/api/v1/hooks/trigger/tr01');
		expect(withToken(url, 'ppt_a-b_c')).toBe(`${url}?token=ppt_a-b_c`);
		expect(withToken(`${url}?x=1`, 'ppt_q')).toBe(`${url}?x=1&token=ppt_q`);
	});

	it('makes a Home Assistant rest_command that YAML parses as intended', () => {
		const y = homeAssistantYaml('Front door "bell"', 'http://pp.local/api/v1/hooks/trigger/t1', 'ppt_X');
		expect(y).toBe(
			[
				'rest_command:',
				'  pixelplus_front_door_bell:',
				'    url: "http://pp.local/api/v1/hooks/trigger/t1"',
				'    method: POST',
				'    headers:',
				'      Authorization: "Bearer ppt_X"'
			].join('\n')
		);
		expect(homeAssistantYaml('A', 'https://x', 't', { https: true })).toContain('verify_ssl: false');
		expect(homeAssistantAction('Front door')).toBe('action: rest_command.pixelplus_front_door');
	});

	it('slugs names', () => {
		expect(slug('Crème brûlée!!')).toBe('creme_brulee');
		expect(slug('🎄')).toBe('trigger');
	});

	it('quotes curl arguments', () => {
		expect(curlCommand('http://pp.local/x', 'ppt_1')).toBe(
			"curl -X POST -H 'Authorization: Bearer ppt_1' http://pp.local/x"
		);
		expect(curlCommand('https://pp.local/x', 'ppt_1', { https: true })).toContain('-k');
	});

	it('orders addresses and hides the internet one unless allowed', () => {
		const all = [
			{ kind: 'internet' as const, label: 'net', base: 'https://lights.example.com' },
			{ kind: 'ip' as const, label: '192.168.1.5', base: 'http://192.168.1.5' },
			{ kind: 'name' as const, label: 'pp.local', base: 'http://pp.local' }
		];
		expect(usableAddresses(all, false).map((a) => a.kind)).toEqual(['name', 'ip']);
		expect(usableAddresses(all, true).map((a) => a.kind)).toEqual(['name', 'ip', 'internet']);
		expect(fallbackAddress('http://127.0.0.1:5173').label).toBe('127.0.0.1:5173');
	});
});

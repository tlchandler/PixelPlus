export type ThemePref = 'dark' | 'light' | 'system';

class Theme {
	/** The theme on screen right now. */
	current = $state<'dark' | 'light'>('dark');
	/** What the viewer picked; "system" follows the phone or computer setting. */
	preference = $state<ThemePref>('system');
	#mq: MediaQueryList | null = null;

	init() {
		const t = document.documentElement.dataset.theme;
		this.current = t === 'light' ? 'light' : 'dark';
		let saved: string | null = null;
		try {
			saved = localStorage.getItem('pp-theme');
		} catch {
			/* ignore */
		}
		this.preference = saved === 'light' || saved === 'dark' ? saved : 'system';
		if (!this.#mq && typeof window !== 'undefined' && window.matchMedia) {
			this.#mq = window.matchMedia('(prefers-color-scheme: light)');
			this.#mq.addEventListener?.('change', () => {
				if (this.preference === 'system') this.#apply(this.#system());
			});
		}
	}
	#system(): 'dark' | 'light' {
		return this.#mq?.matches ? 'light' : 'dark';
	}
	#apply(t: 'dark' | 'light') {
		this.current = t;
		document.documentElement.dataset.theme = t;
		document
			.querySelector('meta[name="theme-color"]')
			?.setAttribute('content', t === 'dark' ? '#0a0b0e' : '#f5f5f3');
	}
	/** Pick a theme (or "system" to follow the device). */
	set(t: ThemePref) {
		this.preference = t;
		try {
			if (t === 'system') localStorage.removeItem('pp-theme');
			else localStorage.setItem('pp-theme', t);
		} catch {
			/* ignore */
		}
		this.#apply(t === 'system' ? this.#system() : t);
	}
	toggle() {
		this.set(this.current === 'dark' ? 'light' : 'dark');
	}
}
export const theme = new Theme();

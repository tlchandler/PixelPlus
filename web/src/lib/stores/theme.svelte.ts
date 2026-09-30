class Theme {
	current = $state<'dark' | 'light'>('dark');
	init() {
		const t = document.documentElement.dataset.theme;
		this.current = t === 'light' ? 'light' : 'dark';
	}
	set(t: 'dark' | 'light') {
		this.current = t;
		document.documentElement.dataset.theme = t;
		document.querySelector('meta[name="theme-color"]')?.setAttribute('content', t === 'dark' ? '#0a0b0e' : '#f5f5f3');
		try {
			localStorage.setItem('pp-theme', t);
		} catch {
			/* ignore */
		}
	}
	toggle() {
		this.set(this.current === 'dark' ? 'light' : 'dark');
	}
}
export const theme = new Theme();

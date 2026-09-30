export interface Toast {
	id: number;
	kind: 'info' | 'success' | 'warning' | 'error';
	message: string;
	detail?: string;
	action?: { label: string; run: () => void | Promise<void> };
	timeout: number;
}

let seq = 1;

class Toasts {
	items = $state<Toast[]>([]);

	push(t: Partial<Toast> & { message: string }): number {
		const id = seq++;
		const toast: Toast = { kind: 'info', timeout: t.action ? 7000 : 4000, ...t, id };
		this.items = [...this.items.slice(-4), toast];
		if (toast.timeout > 0) setTimeout(() => this.dismiss(id), toast.timeout);
		return id;
	}
	dismiss(id: number) {
		this.items = this.items.filter((t) => t.id !== id);
	}
	success(message: string, action?: Toast['action']) {
		return this.push({ kind: 'success', message, action });
	}
	error(message: string, detail?: string) {
		return this.push({ kind: 'error', message, detail, timeout: 6500 });
	}
	info(message: string) {
		return this.push({ kind: 'info', message });
	}
	warn(message: string) {
		return this.push({ kind: 'warning', message, timeout: 6000 });
	}
}

export const toasts = new Toasts();

export interface ConfirmOpts {
	title: string;
	message?: string;
	confirmLabel?: string;
	cancelLabel?: string;
	danger?: boolean;
}

class Dialogs {
	current = $state<(ConfirmOpts & { resolve: (v: boolean) => void }) | null>(null);
	confirm(opts: ConfirmOpts): Promise<boolean> {
		return new Promise((resolve) => {
			this.current?.resolve(false);
			this.current = { ...opts, resolve };
		});
	}
	close(v: boolean) {
		const c = this.current;
		this.current = null;
		c?.resolve(v);
	}
}

export const dialogs = new Dialogs();
export const confirm = (o: ConfirmOpts) => dialogs.confirm(o);

// Pointer-based (mouse + touch) vertical sortable list with keyboard support.
// Items are the container's direct children carrying `data-sort-index`. Drag starts from `.drag-handle`.
export interface SortableOpts {
	onsort: (from: number, to: number) => void;
	handle?: string;
	disabled?: boolean;
}

export function sortable(node: HTMLElement, opts: SortableOpts) {
	let o = opts;
	let dragEl: HTMLElement | null = null;
	let from = -1;
	let to = -1;
	let startY = 0;
	let items: HTMLElement[] = [];
	let rects: DOMRect[] = [];

	function itemsNow() {
		return [...node.children].filter((c) => (c as HTMLElement).dataset.sortIndex != null) as HTMLElement[];
	}

	function onDown(e: PointerEvent) {
		if (o.disabled || e.button > 0) return;
		const h = (e.target as HTMLElement).closest(o.handle ?? '.drag-handle');
		if (!h || !node.contains(h)) return;
		const item = h.closest('[data-sort-index]') as HTMLElement | null;
		if (!item || item.parentElement !== node) return;
		e.preventDefault();
		items = itemsNow();
		rects = items.map((i) => i.getBoundingClientRect());
		dragEl = item;
		from = to = items.indexOf(item);
		startY = e.clientY;
		dragEl.classList.add('dragging');
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp, { once: true });
		window.addEventListener('pointercancel', onUp, { once: true });
	}

	function onMove(e: PointerEvent) {
		if (!dragEl) return;
		const dy = e.clientY - startY;
		dragEl.style.transform = `translateY(${dy}px)`;
		const r = rects[from];
		const center = r.top + r.height / 2 + dy;
		let idx = 0;
		for (let i = 0; i < rects.length; i++) if (center > rects[i].top + rects[i].height / 2) idx = i;
		if (center < rects[0].top + rects[0].height / 2) idx = 0;
		to = idx;
		const h = r.height + (rects[1] ? rects[1].top - rects[0].bottom : 0);
		items.forEach((it, i) => {
			if (it === dragEl) return;
			let shift = 0;
			if (from < to && i > from && i <= to) shift = -h;
			if (from > to && i < from && i >= to) shift = h;
			it.style.transition = 'transform 160ms cubic-bezier(.2,.8,.2,1)';
			it.style.transform = shift ? `translateY(${shift}px)` : '';
		});
	}

	function onUp() {
		window.removeEventListener('pointermove', onMove);
		items.forEach((it) => {
			it.style.transform = '';
			it.style.transition = '';
		});
		dragEl?.classList.remove('dragging');
		dragEl = null;
		if (from !== to && from >= 0) o.onsort(from, to);
	}

	function onKey(e: KeyboardEvent) {
		if (o.disabled) return;
		const h = (e.target as HTMLElement).closest(o.handle ?? '.drag-handle');
		if (!h) return;
		const item = h.closest('[data-sort-index]') as HTMLElement | null;
		if (!item) return;
		const i = Number(item.dataset.sortIndex);
		const n = itemsNow().length;
		if (e.key === 'ArrowUp' && i > 0) {
			e.preventDefault();
			o.onsort(i, i - 1);
			requestAnimationFrame(() =>
				(
					node.querySelector(`[data-sort-index="${i - 1}"] ${o.handle ?? '.drag-handle'}`) as HTMLElement
				)?.focus()
			);
		} else if (e.key === 'ArrowDown' && i < n - 1) {
			e.preventDefault();
			o.onsort(i, i + 1);
			requestAnimationFrame(() =>
				(
					node.querySelector(`[data-sort-index="${i + 1}"] ${o.handle ?? '.drag-handle'}`) as HTMLElement
				)?.focus()
			);
		}
	}

	node.addEventListener('pointerdown', onDown);
	node.addEventListener('keydown', onKey);
	return {
		update(n: SortableOpts) {
			o = n;
		},
		destroy() {
			node.removeEventListener('pointerdown', onDown);
			node.removeEventListener('keydown', onKey);
			window.removeEventListener('pointermove', onMove);
		}
	};
}

export function moveItem<T>(arr: T[], from: number, to: number): T[] {
	const a = [...arr];
	const [x] = a.splice(from, 1);
	a.splice(to, 0, x);
	return a;
}

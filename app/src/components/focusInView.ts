export function focusInView(element: HTMLElement | null) {
  if (!element) return;
  element.scrollIntoView({
    block: 'nearest',
    inline: 'nearest',
    behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth',
  });
  element.focus({ preventScroll: true });
}


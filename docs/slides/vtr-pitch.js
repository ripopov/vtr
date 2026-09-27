const root = document.documentElement, ctl = document.querySelector('.ctl');
const slides = [...document.querySelectorAll('.slide')], buttons = [...document.querySelectorAll('[data-go]')];
let current = 0;
function fit() {
  const present = root.classList.contains('present');
  const w = innerWidth - (present ? 0 : 48), h = innerHeight - (present ? 0 : 24 + ctl.offsetHeight);
  root.style.setProperty('--s', Math.max(0.2, Math.min(w / 1280, h / 720)));
}
function show(i, remember = true) {
  current = Math.max(0, Math.min(slides.length - 1, i));
  slides.forEach((s, n) => { s.hidden = n !== current; });
  buttons.forEach((b, n) => b.setAttribute('aria-current', String(n === current)));
  document.getElementById('said').textContent = slides[current].getAttribute('aria-label');
  if (remember) history.replaceState(null, '', '#' + slides[current].id);
}
function present(on) {
  root.classList.toggle('present', on);
  if (on) root.requestFullscreen?.().catch(() => {});
  else if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  fit();
}
buttons.forEach((b, n) => b.addEventListener('click', () => show(n)));
document.querySelector('[data-present]').addEventListener('click', () => present(true));
document.addEventListener('keydown', e => {
  if (e.altKey || e.ctrlKey || e.metaKey) return;
  const go = {ArrowRight: current + 1, ArrowDown: current + 1, PageDown: current + 1, ' ': current + 1,
    ArrowLeft: current - 1, ArrowUp: current - 1, PageUp: current - 1, Home: 0, End: slides.length - 1, 1: 0, 2: 1}[e.key];
  if (go !== undefined) { e.preventDefault(); show(go); }
  else if (e.key === 'p' || e.key === 'P') present(!root.classList.contains('present'));
  else if (e.key === 'Escape' && root.classList.contains('present')) present(false);
});
document.addEventListener('fullscreenchange', () => {
  if (!document.fullscreenElement && root.classList.contains('present')) { root.classList.remove('present'); fit(); }
});
addEventListener('resize', fit);
addEventListener('hashchange', () => show(Math.max(0, slides.findIndex(s => '#' + s.id === location.hash)), false));
show(Math.max(0, slides.findIndex(s => '#' + s.id === location.hash)), false);
fit();
window.PITCH = {show, present, current: () => current, count: slides.length};
document.fonts.ready.then(() => { window.ready = true; });

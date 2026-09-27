const root = document.documentElement, ctl = document.querySelector('.ctl');
function fit() {
  const present = root.classList.contains('present');
  const w = innerWidth - (present ? 0 : 48), h = innerHeight - (present ? 0 : 24 + ctl.offsetHeight);
  root.style.setProperty('--s', Math.max(0.2, Math.min(w / 1280, h / 720)));
}
function present(on) {
  root.classList.toggle('present', on);
  if (on) root.requestFullscreen?.().catch(() => {});
  else if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  fit();
}
document.querySelector('[data-present]').addEventListener('click', () => present(true));
document.addEventListener('keydown', e => {
  if (e.altKey || e.ctrlKey || e.metaKey) return;
  if (e.key === 'p' || e.key === 'P') present(!root.classList.contains('present'));
  else if (e.key === 'Escape' && root.classList.contains('present')) present(false);
});
document.addEventListener('fullscreenchange', () => {
  if (!document.fullscreenElement && root.classList.contains('present')) { root.classList.remove('present'); fit(); }
});
addEventListener('resize', fit);
fit();
window.PITCH = {present};
document.fonts.ready.then(() => { window.ready = true; });

// Apply the stored theme before CSS to prevent a flash.
function validTheme(theme) {
  return theme === 'light' || theme === 'dark';
}

try {
  const theme = JSON.parse(localStorage.getItem('cidian.preferences'))?.theme;
  if (validTheme(theme)) document.documentElement.dataset.theme = theme;
} catch {}

const systemTheme = matchMedia('(prefers-color-scheme: dark)');
function applySystemTheme() {
  document.documentElement.dataset.systemTheme = systemTheme.matches ? 'dark' : 'light';
}
applySystemTheme();
systemTheme.addEventListener('change', applySystemTheme);

document.addEventListener('click', event => {
  const summary = event.target.closest('details > summary');
  const body = summary?.nextElementSibling;
  if (!body?.classList.contains('disclosure-body') || matchMedia('(prefers-reduced-motion: reduce)').matches) return;
  event.preventDefault();
  const details = summary.parentElement;
  const opening = !details.open || details.classList.contains('closing');
  // Reverse from the current pose without repeated layout reads.
  const [running] = body.getAnimations();
  const progress = running?.effect.getComputedTiming().progress ?? 0;
  const [start, end] = running?.effect.getKeyframes() ?? [];
  const midway = property => parseFloat(start[property]) + (parseFloat(end[property]) - parseFloat(start[property])) * progress;
  const shown = details.open ? 1 : 0;
  running?.cancel();
  details.classList.toggle('closing', !opening);
  details.open = true;
  const height = body.getBoundingClientRect().height;
  const from = running ? midway('height') : height * shown;
  const fade = running ? midway('opacity') : shown;
  const to = opening ? height : 0;
  const style = getComputedStyle(document.documentElement);
  const slow = style.getPropertyValue('--slow').trim();
  const animation = body.animate([
    {height: `${from}px`, opacity: fade, overflow: 'hidden'},
    {height: `${to}px`, opacity: opening ? 1 : 0, overflow: 'hidden'},
  ], {duration: parseFloat(slow) * (slow.endsWith('ms') ? 1 : 1000), easing: style.getPropertyValue('--ease').trim(), fill: opening ? 'none' : 'forwards'});
  // Hold the collapsed pose until closing to prevent a flash.
  if (!opening) {
    animation.finished.then(() => {
      details.open = false;
      details.classList.remove('closing');
      animation.cancel();
    }, () => {});
  }
});

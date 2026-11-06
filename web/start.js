// Keep this handler at a stable URL so old HTML can recover after a deploy.
let startupFailed = false;
document.addEventListener('error', event => {
  const asset = event.target;
  if (!asset.matches?.('script[src], link[rel="stylesheet"][href]')) return;
  const url = new URL(asset.src || asset.href);
  if (url.origin !== location.origin || !/\.[a-f0-9]{16}\.(js|css)$/.test(url.pathname)) return;
  startupFailed = true;
  try {
    if (sessionStorage.getItem('cidian.module-reload')) return;
    sessionStorage.setItem('cidian.module-reload', '1');
  } catch {
    // Without a saved guard, reloading could loop.
    return;
  }
  location.reload();
}, true);

window.addEventListener('load', () => {
  if (startupFailed) return;
  try {
    sessionStorage.removeItem('cidian.module-reload');
  } catch {}
});

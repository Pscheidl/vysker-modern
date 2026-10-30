(() => {
  const ios = /iPhone|iPod/i.test(navigator.userAgent);
  const android = /Android.+Mobile/i.test(navigator.userAgent);
  if (!ios && !android) return;

  let installPrompt = null;
  let installed = false;
  const isStandalone = () => installed || navigator.standalone === true ||
    matchMedia('(display-mode: standalone)').matches || matchMedia('(display-mode: fullscreen)').matches;
  const update = () => {
    const link = document.querySelector('[data-app-install-link]');
    const button = document.querySelector('[data-app-install-button]');
    if (link) link.hidden = isStandalone();
    if (button) button.hidden = isStandalone() || !installPrompt;
  };

  window.addEventListener('beforeinstallprompt', event => {
    if (isStandalone()) return;
    event.preventDefault();
    installPrompt = event;
    update();
  });
  window.addEventListener('appinstalled', () => {
    installed = true;
    installPrompt = null;
    update();
  });
  document.addEventListener('click', async event => {
    const button = event.target.closest('[data-app-install-button]');
    if (!button || !installPrompt) return;
    const prompt = installPrompt;
    installPrompt = null;
    update();
    try { await prompt.prompt(); } catch (_) { /* The manual steps remain available. */ }
  });
  const start = () => {
    update();
    new MutationObserver(update).observe(document.body, {childList: true, subtree: true});
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, {once: true});
  else start();
})();

// Private subscription pages use native forms and do not hydrate the public app.
(() => {
  const switcher = document.querySelector('[data-subscription-theme]');
  if (!switcher) return;
  switcher.hidden = false;
  const update = () => {
    for (const button of switcher.querySelectorAll('[data-theme-choice]')) {
      button.setAttribute('aria-pressed', String(button.dataset.themeChoice === document.documentElement.dataset.theme));
    }
  };
  switcher.addEventListener('click', event => {
    const button = event.target.closest('[data-theme-choice]');
    if (!button) return;
    const theme = button.dataset.themeChoice;
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('vysker-theme', theme); } catch (_) {}
    update();
  });
  update();
})();

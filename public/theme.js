/* Pouze výběr motivu před prvním vykreslením. Ovládání zajišťuje Leptos. */
(() => {
  try {
    const saved = localStorage.getItem('vysker-theme');
    const theme = saved === 'light' || saved === 'dark'
      ? saved : (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
    document.documentElement.dataset.theme = theme;
  } catch (_) {
    document.documentElement.dataset.theme = 'light';
  }
})();

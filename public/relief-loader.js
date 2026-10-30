// The scene is loaded only when the visitor opens its page, including SPA navigation.
(() => {
  let mounted = null;
  let cleanup = null;
  let loading = null;
  const update = () => {
    const container = document.querySelector('[data-relief][data-relief-ready]');
    if (mounted && mounted !== container) {
      if (cleanup) cleanup();
      mounted = cleanup = null;
    }
    if (!container || container === mounted) return;
    mounted = container;
    const url = container.dataset.relief;
    loading = import(url);
    loading.then(module => {
      if (mounted === container && container.isConnected) cleanup = module.mount(container);
    }).catch(error => {
      console.error('3D relief:', error);
    });
  };
  new MutationObserver(update).observe(document.documentElement, {childList: true, subtree: true, attributes: true, attributeFilter: ['data-relief-ready']});
  update();
})();

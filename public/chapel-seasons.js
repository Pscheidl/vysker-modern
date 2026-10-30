// The artwork follows the Prague calendar, independent of the visitor's clock zone.
(() => {
  const labels = { spring: 'spring', summer: 'summer', autumn: 'autumn', winter: 'winter', christmas: 'christmas' };
  let mounted = null;
  let currentSeason = null;

  function seasonForDate(year, month, day) {
    const fourthAdvent = new Date(Date.UTC(year, 11, 24));
    fourthAdvent.setUTCDate(24 - fourthAdvent.getUTCDay());
    const firstAdvent = new Date(fourthAdvent.getTime() - 21 * 86400000);
    const today = Date.UTC(year, month - 1, day);
    if (today >= firstAdvent.getTime() && month <= 12) return labels.christmas;
    if (month >= 3 && month <= 5) return labels.spring;
    if (month >= 6 && month <= 8) return labels.summer;
    if (month >= 9 && month <= 11) return labels.autumn;
    return labels.winter;
  }

  function currentPragueSeason() {
    const parts = new Intl.DateTimeFormat('en-US', {
      timeZone: 'Europe/Prague', year: 'numeric', month: 'numeric', day: 'numeric'
    }).formatToParts(new Date());
    const part = name => Number(parts.find(item => item.type === name).value);
    return seasonForDate(part('year'), part('month'), part('day'));
  }

  function addWeather(container, season) {
    const previous = container.querySelector('.chapel-weather');
    if (previous) previous.remove();
    const snowy = season === 'winter' || season === 'christmas';
    if (!snowy && season !== 'autumn') return;
    const layer = document.createElement('div');
    layer.className = 'chapel-weather';
    layer.setAttribute('aria-hidden', 'true');
    const colors = ['#c87821', '#be4e24', '#d7a32e', '#99502c'];
    const count = snowy ? 32 : 16;
    for (let i = 0; i < count; i++) {
      const particle = document.createElement('span');
      particle.className = `chapel-particle ${snowy ? 'chapel-snow' : 'chapel-leaf'}`;
      const size = snowy ? 2 + (i * 7 % 5) * .55 : 6 + (i * 7 % 6);
      particle.style.setProperty('--x', `${(i * 37.13 + 3) % 100}%`);
      particle.style.setProperty('--size', `${size}px`);
      particle.style.setProperty('--opacity', `${.55 + (i % 5) * .08}`);
      particle.style.setProperty('--duration', `${8 + (i * 13 % 70) / 10}s`);
      particle.style.setProperty('--delay', `${-(i * 3.41 % 13)}s`);
      particle.style.setProperty('--drift', `${-25 + (i * 17 % 65)}px`);
      particle.style.setProperty('--color', colors[i % colors.length]);
      layer.appendChild(particle);
    }
    container.appendChild(layer);
  }

  function update() {
    const container = document.querySelector('.terrain-chapel');
    if (!container) { mounted = null; currentSeason = null; return; }
    const season = currentPragueSeason();
    if (container === mounted && season === currentSeason) return;
    const light = container.querySelector('.terrain-light');
    const dark = container.querySelector('.terrain-dark');
    if (!light || !dark) return;
    const directory = new URL('.', light.src);
    const filename = theme => season === 'summer'
      ? `chapel-hura-${theme}.webp`
      : `chapel-${season}-${theme}.webp`;
    light.src = new URL(filename('light'), directory).href;
    dark.src = new URL(filename('dark'), directory).href;
    mounted = container;
    currentSeason = season;
    addWeather(container, season);
  }

  new MutationObserver(update).observe(document.documentElement, { childList: true, subtree: true });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) update(); });
  setInterval(update, 60000);
  update();
})();

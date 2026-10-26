// TifMrcView website behaviour shared by the home page and the plugin market:
// compact nav on scroll, scroll reveal, screenshot tabs, lightbox, GitHub stars.
(function () {
  var nav = document.querySelector('.nav');
  if (nav) {
    var onScroll = function () { nav.classList.toggle('stuck', window.scrollY > 8); };
    window.addEventListener('scroll', onScroll, { passive: true });
    onScroll();
  }

  // Fade/slide up once an element enters the viewport. Without JS (or without
  // IntersectionObserver) everything is simply visible.
  var rv = document.querySelectorAll('.rv');
  if ('IntersectionObserver' in window) {
    var io = new IntersectionObserver(function (es) {
      es.forEach(function (e) { if (e.isIntersecting) { e.target.classList.add('in'); io.unobserve(e.target); } });
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.08 });
    rv.forEach(function (el) { io.observe(el); });
  } else {
    rv.forEach(function (el) { el.classList.add('in'); });
  }

  // Segmented tabs that cross-fade between screenshots.
  document.querySelectorAll('[role="tablist"]').forEach(function (list) {
    var tabs = Array.prototype.slice.call(list.querySelectorAll('[role="tab"]'));
    function select(tab, focus) {
      tabs.forEach(function (t) {
        var on = t === tab;
        t.setAttribute('aria-selected', on ? 'true' : 'false');
        t.tabIndex = on ? 0 : -1;
        document.getElementById(t.getAttribute('aria-controls')).hidden = !on;
      });
      if (focus) tab.focus();
    }
    tabs.forEach(function (t, i) {
      t.addEventListener('click', function () { select(t); });
      t.addEventListener('keydown', function (e) {
        var d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
        if (d) { e.preventDefault(); select(tabs[(i + d + tabs.length) % tabs.length], true); }
      });
    });
  });

  // Lightbox for the full-size screenshots.
  var lb = document.getElementById('lb');
  if (lb) {
    var im = lb.querySelector('img'), last = null;
    var open = function (b) {
      last = b;
      im.src = b.getAttribute('data-full');
      im.alt = b.querySelector('img').alt;
      lb.hidden = false; lb.classList.add('open');
      document.body.style.overflow = 'hidden';
      lb.querySelector('button').focus();
    };
    var close = function () {
      lb.classList.remove('open'); lb.hidden = true; im.removeAttribute('src');
      document.body.style.overflow = '';
      if (last) last.focus();
    };
    document.querySelectorAll('.shot[data-full]').forEach(function (b) {
      b.addEventListener('click', function () { open(b); });
    });
    lb.addEventListener('click', close);
    document.addEventListener('keydown', function (e) { if (e.key === 'Escape' && lb.classList.contains('open')) close(); });
  }

  // Live GitHub star count; the counters stay empty (hidden) if it fails or is 0.
  var counters = document.querySelectorAll('.starn');
  if (counters.length && window.fetch) {
    fetch('https://api.github.com/repos/Leehow/TifMrcView').then(function (r) { return r.ok ? r.json() : null; }).then(function (d) {
      if (!d || typeof d.stargazers_count !== 'number' || d.stargazers_count < 1) return;
      var n = d.stargazers_count.toLocaleString();
      counters.forEach(function (c) { c.textContent = n; });
    }).catch(function () {});
  }
})();

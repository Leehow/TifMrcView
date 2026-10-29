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

  // Render normal links first, then prefer the direct host if its JSON is reachable.
  if (document.getElementById('cta') && window.fetch && window.AbortController) {
    var direct = 'https://dl.tifmrcview.com:9443/tifmrcview/';
    var probing = false, directReady = false;
    function directLinks() {
      document.querySelectorAll('#download a[download], #cta[download]').forEach(function (a) {
        var file = a.getAttribute('href');
        if (file && !/^(?:[a-z]+:|\/|#)/i.test(file)) a.href = direct + file;
      });
    }
    function preferDirect() {
      if (directReady) { directLinks(); return; }
      if (probing) return;
      probing = true;
      var controller = new AbortController();
      var timer = setTimeout(function () { controller.abort(); }, 3000);
      fetch(direct + 'downloads.json', { signal: controller.signal, cache: 'no-cache' })
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (d) { if (d && d.files) { directReady = true; directLinks(); } })
        .catch(function () {})
        .finally(function () { clearTimeout(timer); });
    }
    document.addEventListener('downloads-rendered', preferDirect);
    if (document.querySelector('#download a[download]')) preferDirect();
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

  // Live demo in the hero: the real viewer UI (demo/app/, built from ui/ by
  // scripts/build-site-demo.mjs) in an iframe. It is laid out at least
  // MIN_W wide and scaled down to fit, and only takes the mouse after a
  // click, so scrolling the page over it still scrolls the page.
  var demo = document.getElementById('demo');
  if (demo) {
    var frame = document.getElementById('demo-frame');
    var shield = document.getElementById('demo-shield');
    var full = document.getElementById('demo-full');
    var MIN_W = 940;
    var demoLang = function () { return window.TMV ? window.TMV.lang() : 'en'; };
    var src = function () { return 'demo/app/?lang=' + demoLang(); };
    var fit = function () {
      var w = demo.clientWidth, h = demo.clientHeight;
      var k = Math.min(1, w / MIN_W);
      frame.style.width = w / k + 'px';
      frame.style.height = h / k + 'px';
      frame.style.transform = k < 1 ? 'scale(' + k + ')' : '';
    };
    fit();
    window.addEventListener('resize', fit);
    frame.addEventListener('load', function () { if (frame.src) frame.classList.add('ready'); });
    var start = function () { frame.src = src(); };
    if (document.readyState === 'complete') start(); else window.addEventListener('load', start);
    if (window.TMV) window.TMV.onLang(function () {
      full.href = src();
      if (frame.src) { frame.classList.remove('ready'); frame.src = src(); }
    });
    shield.addEventListener('click', function () {
      shield.hidden = true;
      frame.tabIndex = 0;
      frame.focus();
    });
    demo.addEventListener('mouseleave', function () { shield.hidden = false; });
  }
})();

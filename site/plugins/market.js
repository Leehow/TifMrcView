(function () {
  var box = document.getElementById('plugins');
  var data = null, failed = false;

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function size(n) {
    return n >= 1048576 ? (n / 1048576).toFixed(1) + ' MB' : Math.max(1, Math.round(n / 1024)) + ' KB';
  }
  // Text for the current language: its own entry, else English, else the Chinese original.
  function field(p, key, lang) {
    var tr = p.i18n || {};
    if (lang !== 'zh') {
      if (tr[lang] && tr[lang][key]) return tr[lang][key];
      if (tr.en && tr.en[key]) return tr.en[key];
    }
    return p[key] || '';
  }
  function paragraphs(p, lang) {
    var d = p.details || {};
    var list = (lang !== 'zh' && (d[lang] || d.en)) || d.zh || d.en || [];
    return Array.isArray(list) ? list : [String(list)];
  }

  // Hands a tifmrcview:// link to the app. Browsers say nothing when no app
  // handles the scheme, so if the page still has focus after a moment
  // (no app took over, no "open TifMrcView?" prompt), show how to get one.
  var frame = null;
  function launch(url, hint) {
    var left = false;
    function away() { left = true; }
    function vis() { if (document.hidden) left = true; }
    window.addEventListener('blur', away);
    document.addEventListener('visibilitychange', vis);
    hint.hidden = true;
    if (/Firefox\//.test(navigator.userAgent)) {
      // Firefox replaces the page with an error for unknown schemes; keep that in a hidden frame.
      if (!frame) { frame = document.createElement('iframe'); frame.hidden = true; document.body.appendChild(frame); }
      frame.src = url;
    } else {
      location.href = url;
    }
    setTimeout(function () {
      window.removeEventListener('blur', away);
      document.removeEventListener('visibilitychange', vis);
      if (!left && !document.hidden && document.hasFocus()) hint.hidden = false;
    }, 1500);
  }

  function card(p, lang) {
    var T = window.TMV.t;
    var c = el('article', 'card plug');
    c.appendChild(el('h3', null, field(p, 'name', lang)));
    var tags = el('div', 'tags');
    if (p.builtin) tags.appendChild(el('span', 'tag green', T('pl_builtin')));
    if (p.native) {
      var n = el('span', 'tag orange', T('pl_native'));
      n.title = T('pl_native_tip');
      tags.appendChild(n);
    }
    if (tags.children.length) c.appendChild(tags);

    var meta = [T('pl_ver', { v: p.version })];
    if (p.author) meta.push(T('pl_by', { a: p.author }));
    if (!p.builtin && p.size) meta.push(size(p.size));
    c.appendChild(el('p', 'meta2', meta.join(' · ')));
    if (p.minAppVersion) c.appendChild(el('p', 'meta2', T('pl_need', { v: p.minAppVersion })));

    c.appendChild(el('p', 'desc', field(p, 'description', lang)));

    var paras = paragraphs(p, lang);
    if (paras.length) {
      var d = el('details');
      d.appendChild(el('summary', null, T('pl_more')));
      paras.forEach(function (t) { d.appendChild(el('p', null, t)); });
      c.appendChild(d);
    }

    var act = el('div', 'act');
    if (p.builtin) {
      act.appendChild(el('div', 'btn off', T('pl_builtin_note')));
    } else {
      var hint = el('p', 'nohandler');
      hint.hidden = true;
      hint.setAttribute('role', 'status');
      hint.innerHTML = T('pl_nohandler');
      var ins = el('a', 'btn primary', T('pl_install'));
      ins.href = 'tifmrcview://install-plugin?id=' + encodeURIComponent(p.id) + '&version=' + encodeURIComponent(p.version);
      ins.title = T('pl_install_tip');
      ins.addEventListener('click', function (e) { e.preventDefault(); launch(ins.href, hint); });
      var a = el('a', 'btn sec', T('pl_download'));
      a.href = p.file;
      a.setAttribute('download', '');
      act.appendChild(ins);
      act.appendChild(a);
      act.appendChild(hint);
    }
    c.appendChild(act);
    return c;
  }

  function render() {
    var T = window.TMV.t, lang = window.TMV.lang();
    box.innerHTML = '';
    if (failed) { var m = el('div', 'msg', T('pl_fail')); m.style.gridColumn = '1/-1'; box.appendChild(m); return; }
    if (!data) { var l = el('div', 'msg', T('pl_loading')); l.style.gridColumn = '1/-1'; box.appendChild(l); return; }
    var list = data.plugins || [];
    if (!list.length) { var e = el('div', 'msg', T('pl_empty')); e.style.gridColumn = '1/-1'; box.appendChild(e); return; }
    // Downloadable plugins first, the built-in one last.
    list.slice().sort(function (a, b) { return (a.builtin ? 1 : 0) - (b.builtin ? 1 : 0); })
      .forEach(function (p) { box.appendChild(card(p, lang)); });
  }

  window.TMV.onLang(render);
  fetch('plugins.json', { cache: 'no-cache' })
    .then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); })
    .then(function (d) { data = d; render(); })
    .catch(function () { failed = true; render(); });
})();

// Shared behaviour: theme toggle, code highlighting, diagnostics search.
(() => {
  const root = document.documentElement;

  // Theme: follows the system until the visitor picks one.
  const btn = document.querySelector('.theme');
  if (btn) {
    const dark = () => root.dataset.theme ? root.dataset.theme === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches;
    const label = () => btn.setAttribute('aria-label', dark() ? 'Use light theme' : 'Use dark theme');
    label();
    btn.addEventListener('click', () => {
      root.dataset.theme = dark() ? 'light' : 'dark';
      try { localStorage.setItem('theme', root.dataset.theme); } catch {}
      label();
    });
  }

  // Highlighting. Write code as plain escaped text in <pre data-lang="sisu|sh|out">.
  const esc = s => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  const KW = 'fn|let|mut|var|const|if|else|while|for|in|return|break|continue|class|enum|match|is|self|true|false|None|todo';
  const sisu = new RegExp(`(\\/\\/.*$)|("(?:[^"\\\\]|\\\\.)*")|\\b(${KW})\\b|\\b([A-Z]\\w*|string|f64|i64|bool|char|u8)\\b|\\b(\\d[\\d_]*(?:\\.\\d+)?)\\b`, 'g');
  const hl = line => esc(line).replace(sisu, (m, c, s, k, t, n) =>
    c ? `<span class="cm">${c}</span>` : s ? `<span class="st">${s}</span>` : k ? `<span class="kw">${k}</span>` : t ? `<span class="ty">${t}</span>` : `<span class="nu">${n}</span>`);
  const sh = line => line.startsWith('$')
    ? `<span class="p">$</span>${esc(line.slice(1)).replace(/(#.*)$/, '<span class="cm">$1</span>')}`
    : esc(line);
  document.querySelectorAll('pre[data-lang]').forEach(pre => {
    const f = { sisu: hl, sh, out: esc }[pre.dataset.lang] || esc;
    pre.innerHTML = pre.textContent.replace(/^\n|\n$/g, '').split('\n').map(l => `<span class="ln">${f(l)}</span>`).join('');
  });

  // Diagnostics index search: a code prefix, or a prefix of a title word for every word typed.
  // Words are split at punctuation too, so `nan` finds "f64.nan".
  const words = s => s.split(/[^\p{L}\p{N}_]+/u).filter(Boolean);
  const rows = [...document.querySelectorAll('li[data-code]')];
  const input = document.querySelector('.dx input[name="q"]');
  if (rows.length && input) {
    const none = document.querySelector('.dx-none');
    const filter = () => {
      const query = input.value.trim();
      const code = query.toUpperCase();
      const typed = words(query.toLowerCase());
      let shown = 0;
      rows.forEach(li => {
        const title = words(li.dataset.words);
        li.hidden = !(li.dataset.code.startsWith(code) || (typed.length && typed.every(t => title.some(w => w.startsWith(t)))));
        if (!li.hidden) shown++;
      });
      document.querySelectorAll('.dx-index ul.rows').forEach(ul => {
        ul.hidden = !ul.querySelector('li:not([hidden])');
        const heading = ul.previousElementSibling;
        heading.hidden = ul.hidden;
        // The sidebar link to a hidden section would jump to nothing.
        const link = document.querySelector(`.dx aside a[href="#${heading.id}"]`);
        if (link) link.parentElement.hidden = ul.hidden;
      });
      none.textContent = shown ? '' : `No diagnostic matches “${query}”.`;
    };
    input.value = new URLSearchParams(location.search).get('q') || '';
    input.addEventListener('input', filter);
    input.form.addEventListener('submit', e => {
      e.preventDefault();
      const li = rows.find(li => li.dataset.code === input.value.trim().toUpperCase());
      if (li) location.href = li.querySelector('a').href;
    });
    filter();
  }
})();

// Shared behaviour: theme toggle, code highlighting.
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
})();

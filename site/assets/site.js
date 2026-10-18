// Shared behaviour: theme toggle, code highlighting, status board.
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

  // Status board, from docs/roadmap.md at commit f372ddd. w: works today; p: planned.
  const AREAS = [
    { name: 'Language', note: 'No stability promise yet: any change may break programs.', items: [
      ['w', 'Nothing changes unless marked <code>mut</code>'], ['w', 'Functions, <code>if</code> as a value, loops'], ['w', 'Classes with fields and methods'],
      ['w', 'Optionals instead of null'], ['w', 'Enums that carry data, including recursive ones'], ['w', '<code>match</code> that must cover every case'],
      ['w', 'Integers that panic on overflow; IEEE 754 floats'], ['w', 'UTF-8 strings and formatting'], ['w', 'Arrays that behave like values'], ['w', 'Compile-time constants'],
      ['p', 'Optionals that nest, such as <code>i64??</code>'], ['p', 'Generics and interfaces'],
      ['p', 'Errors with <code>Result</code> and <code>?</code>'], ['p', '<code>weak</code> references'], ['p', 'Closures and iterators'],
      ['p', 'Records, named and default arguments'], ['p', 'Modules'], ['p', 'Tests in the source file'],
      ['p', 'More number types'], ['p', 'Concurrency'] ] },
    { name: 'Standard library', note: 'Still small: many everyday tools are missing.', items: [
      ['w', 'Printing and reading lines'], ['w', 'Program arguments'], ['w', 'Text as characters, bytes and words'],
      ['w', 'Float math'], ['w', '<code>len</code>, <code>sum</code> and <code>contains</code> on arrays'],
      ['p', 'Lists and maps'], ['p', 'Files, environment and exit codes'] ] },
    { name: 'Compiler', items: [
      ['w', 'Native x86-64 Linux code through LLVM'], ['w', 'Optimized builds with <code>-O2</code>'], ['w', 'Reference counting, no garbage collector'],
      ['w', 'Panics that name the source line'], ['w', 'Front end fuzzed nightly'],
      ['p', 'MIR, to remove reference-count updates that cancel out'], ['p', 'Debug info for <code>gdb</code>'],
      ['p', 'ARM64 Linux, macOS, Windows'], ['p', 'Cross-compiling'], ['p', 'Calling C libraries'], ['p', 'Fuzzing for wrong answers'], ['p', 'Lexer and parser in Sisu'] ] },
    { name: 'Toolchain', items: [
      ['w', '<code>sisuc</code>: build, check, show IR'], ['w', 'Benchmarks on every pull request that touches the code'],
      ['p', 'One <code>sisu</code> command: build, run, test, fmt'], ['p', 'Install without cloning'], ['p', 'Packages from a path or Git'] ] },
    { name: 'Developer experience', items: [
      ['w', 'Errors that point at your code'], ['w', '<code>--explain</code> for all 142 codes'], ['w', 'Fixes marked safe or unsafe'], ['w', 'JSON output for tools'],
      ['p', 'Formatter'], ['p', 'Language server'], ['p', 'VS Code highlighting'], ['p', 'More lints'], ['p', 'Recovery from syntax errors inside a function'], ['p', 'Leak finder (<code>--leaks</code>)'],
      ['p', 'Doc comments'], ['p', 'A language reference'] ] },
  ];
  window.SISU_AREAS = AREAS;
  const board = document.getElementById('areas');
  if (board) {
    const li = xs => xs.map(t => `<li>${t}</li>`).join('');
    board.innerHTML = AREAS.map((A, i) => {
      const w = A.items.filter(x => x[0] === 'w').map(x => x[1]);
      const p = A.items.filter(x => x[0] !== 'w').map(x => x[1]);
      return `<details class="area"${i === 0 || board.dataset.open === 'all' ? ' open' : ''}><summary>
        <h3>${A.name}</h3>
        <div class="bar" aria-hidden="true"><i class="ok" style="flex:${w.length}"></i><i class="plan" style="flex:${p.length}"></i></div>
        <span class="counts"><b class="ok">${w.length}</b> work today · <b class="plan">${p.length}</b> planned</span>
        <svg class="chev" aria-hidden="true"><use href="#chev"/></svg></summary>
        <div class="detail">
          <div class="wl"><h4>Works today</h4><ul>${li(w)}</ul></div>
          <div><h4>Planned</h4><ul class="pl">${li(p)}</ul></div>
          ${A.note ? `<p class="note">${A.note}</p>` : ''}
        </div></details>`;
    }).join('');
  }
})();

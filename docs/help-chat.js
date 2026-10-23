/* Sweepwise Help: the chat bubble on the website. It does nothing until the page gives it a back-end address
   (the data-endpoint on its <script> tag), so a site without the back end never shows a broken chat. */
(function () {
  var script = document.currentScript;
  var endpoint = script && script.getAttribute('data-endpoint');
  if (!endpoint) return;
  endpoint = endpoint.replace(/\/$/, '');

  var ISSUES = 'https://github.com/AlbertanCoder/sweepwise/issues/new';
  var history = [];
  var pending = null;
  var busy = false;

  var css = document.createElement('style');
  css.textContent =
    '.hc-open{position:fixed;right:18px;bottom:18px;z-index:20;display:flex;align-items:center;gap:8px;padding:12px 18px;border-radius:999px;border:0;cursor:pointer;font:600 15px/1 inherit;color:var(--onteal,#032421);background:linear-gradient(180deg,var(--teal,#2dd4bf),var(--teal2,#14b8a6));box-shadow:0 10px 30px rgba(0,0,0,.4)}' +
    '.hc-open:hover{filter:brightness(1.06)}' +
    '.hc-panel{position:fixed;right:18px;bottom:18px;z-index:21;width:min(380px,calc(100vw - 36px));height:min(560px,calc(100vh - 36px));display:flex;flex-direction:column;border-radius:16px;overflow:hidden;background:var(--card,#121c33);border:1px solid var(--line,#22304f);box-shadow:0 24px 60px rgba(0,0,0,.55)}' +
    '.hc-panel[hidden]{display:none}' +
    '.hc-head{display:flex;align-items:center;justify-content:space-between;padding:12px 14px;border-bottom:1px solid var(--line,#22304f);font-weight:650}' +
    '.hc-x{background:none;border:0;color:var(--muted,#9aa8c3);font-size:22px;line-height:1;cursor:pointer;padding:2px 6px}' +
    '.hc-note{margin:0;padding:8px 14px;font-size:12.5px;color:var(--muted,#9aa8c3);background:var(--card2,#16223d)}' +
    '.hc-log{flex:1;overflow:auto;padding:12px 14px;display:flex;flex-direction:column;gap:10px}' +
    '.hc-m{max-width:88%;padding:9px 12px;border-radius:12px;font-size:14.5px;line-height:1.5;white-space:pre-wrap;overflow-wrap:anywhere}' +
    '.hc-bot{align-self:flex-start;background:var(--card2,#16223d);color:var(--text,#e7eef9)}' +
    '.hc-me{align-self:flex-end;background:rgba(45,212,191,.18);color:var(--text,#e7eef9)}' +
    '.hc-card{align-self:stretch;border:1px solid var(--line,#22304f);border-radius:12px;padding:10px 12px;font-size:13.5px}' +
    '.hc-card p{margin:0 0 8px;color:var(--muted,#9aa8c3)}' +
    '.hc-card q{display:block;margin:0 0 10px;color:var(--text,#e7eef9);quotes:none}' +
    '.hc-row{display:flex;gap:8px;flex-wrap:wrap}' +
    '.hc-btn{padding:8px 14px;border-radius:10px;border:1px solid var(--line,#22304f);background:var(--card2,#16223d);color:var(--text,#e7eef9);font:600 13.5px/1 inherit;cursor:pointer}' +
    '.hc-btn.go{border:0;color:var(--onteal,#032421);background:linear-gradient(180deg,var(--teal,#2dd4bf),var(--teal2,#14b8a6))}' +
    '.hc-form{display:flex;gap:8px;padding:10px 12px;border-top:1px solid var(--line,#22304f)}' +
    '.hc-in{flex:1;min-width:0;padding:10px 12px;border-radius:10px;border:1px solid var(--line,#22304f);background:var(--bg,#0a1020);color:var(--text,#e7eef9);font:inherit;font-size:14.5px}' +
    '@media (prefers-reduced-motion:no-preference){.hc-panel{animation:hcin .16s ease-out}@keyframes hcin{from{opacity:0;transform:translateY(8px)}to{opacity:1;transform:none}}}';
  document.head.appendChild(css);

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  var openBtn = el('button', 'hc-open', 'Ask Sweepwise Help');
  openBtn.type = 'button';
  openBtn.setAttribute('aria-haspopup', 'dialog');
  openBtn.setAttribute('aria-expanded', 'false');

  var panel = el('div', 'hc-panel');
  panel.hidden = true;
  panel.setAttribute('role', 'dialog');
  panel.setAttribute('aria-label', 'Sweepwise Help chat');
  var head = el('div', 'hc-head');
  head.appendChild(el('span', null, 'Sweepwise Help'));
  var closeBtn = el('button', 'hc-x', '×');
  closeBtn.type = 'button';
  closeBtn.setAttribute('aria-label', 'Close the chat');
  head.appendChild(closeBtn);
  var note = el('p', 'hc-note', 'This is an AI assistant. Your messages go to a Sweepwise server, OpenRouter and an AI model (DeepSeek) to write the answer, and Sweepwise does not store them. Please don’t share personal details.');
  var log = el('div', 'hc-log');
  log.setAttribute('aria-live', 'polite');
  var form = el('form', 'hc-form');
  var input = el('input', 'hc-in');
  input.type = 'text';
  input.maxLength = 500;
  input.placeholder = 'Ask a question about Sweepwise';
  input.setAttribute('aria-label', 'Your question');
  var sendBtn = el('button', 'hc-btn go', 'Send');
  sendBtn.type = 'submit';
  form.appendChild(input);
  form.appendChild(sendBtn);
  panel.appendChild(head);
  panel.appendChild(note);
  panel.appendChild(log);
  panel.appendChild(form);
  document.body.appendChild(openBtn);
  document.body.appendChild(panel);

  function say(text, who) {
    var m = el('div', 'hc-m ' + (who === 'me' ? 'hc-me' : 'hc-bot'), text);
    log.appendChild(m);
    log.scrollTop = log.scrollHeight;
    return m;
  }

  function fallback(text) {
    var m = say(text + ' You can also ', 'bot');
    var a = el('a', null, 'report a problem on GitHub');
    a.href = ISSUES;
    a.target = '_blank';
    a.rel = 'noopener';
    m.appendChild(a);
    m.appendChild(document.createTextNode('.'));
  }

  function offerHandoff(summary) {
    pending = summary;
    var card = el('div', 'hc-card');
    card.appendChild(el('p', null, 'Send this to the maker? It will be posted publicly on GitHub, with personal details removed, as:'));
    card.appendChild(el('q', null, summary));
    var row = el('div', 'hc-row');
    var yes = el('button', 'hc-btn go', 'Send it');
    var no = el('button', 'hc-btn', 'No thanks');
    yes.type = no.type = 'button';
    row.appendChild(yes);
    row.appendChild(no);
    card.appendChild(row);
    log.appendChild(card);
    log.scrollTop = log.scrollHeight;
    no.addEventListener('click', function () { card.remove(); pending = null; say('No problem. Nothing was sent.', 'bot'); });
    yes.addEventListener('click', function () {
      yes.disabled = no.disabled = true;
      fetch(endpoint + '/handoff', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ consent: true, summary: pending, source: 'website help chat' }) })
        .then(function (r) { return r.ok ? r.json() : Promise.reject(r); })
        .then(function (d) {
          card.remove();
          var m = say('Sent. You can follow it here: ', 'bot');
          var a = el('a', null, d.issue_url);
          a.href = d.issue_url;
          a.target = '_blank';
          a.rel = 'noopener';
          m.appendChild(a);
        })
        .catch(function () { card.remove(); fallback('I couldn’t send that just now.'); });
    });
  }

  form.addEventListener('submit', function (e) {
    e.preventDefault();
    var text = input.value.trim();
    if (!text || busy) return;
    input.value = '';
    say(text, 'me');
    history.push({ role: 'user', content: text });
    if (history.length > 8) history = history.slice(-8);
    busy = true;
    sendBtn.disabled = true;
    var wait = say('…', 'bot');
    fetch(endpoint + '/chat', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ messages: history }) })
      .then(function (r) { return r.json().then(function (d) { return { ok: r.ok, d: d }; }); })
      .then(function (x) {
        wait.remove();
        if (!x.ok) {
          history.pop();
          fallback(x.d && x.d.error === 'limit' ? 'That’s all the questions I can take from one visitor today.' : 'The chat is resting right now.');
          return;
        }
        say(x.d.reply, 'bot');
        history.push({ role: 'assistant', content: x.d.reply });
        if (x.d.handoff && x.d.handoff.summary) offerHandoff(x.d.handoff.summary);
      })
      .catch(function () { wait.remove(); history.pop(); fallback('I couldn’t reach the chat.'); })
      .then(function () { busy = false; sendBtn.disabled = false; input.focus(); });
  });

  function open() {
    panel.hidden = false;
    openBtn.hidden = true;
    openBtn.setAttribute('aria-expanded', 'true');
    if (!log.children.length) say('Hi, I’m the Sweepwise help assistant. Ask me anything about what Sweepwise does, how to get it, or what to do if something isn’t working.', 'bot');
    input.focus();
  }
  function close() {
    panel.hidden = true;
    openBtn.hidden = false;
    openBtn.setAttribute('aria-expanded', 'false');
    openBtn.focus();
  }
  openBtn.addEventListener('click', open);
  closeBtn.addEventListener('click', close);
  panel.addEventListener('keydown', function (e) { if (e.key === 'Escape') close(); });
})();

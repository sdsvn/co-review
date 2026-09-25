/* eslint-disable max-len */
/** The phone view: a single self-contained page (no build step), talking to /api/m. */

export const MOBILE_ICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#312E81"/><stop offset="1" stop-color="#1E1B4B"/></linearGradient></defs><rect width="64" height="64" rx="14" fill="url(#bg)"/><path d="M15 11h24a6 6 0 0 1 6 6v12a6 6 0 0 1-6 6H22l-7 6v-6a6 6 0 0 1-6-6V17a6 6 0 0 1 6-6z" fill="#EEF2FF"/><rect x="15" y="17" width="15" height="3" rx="1.5" fill="#6366F1"/><rect x="15" y="22.5" width="21" height="3" rx="1.5" fill="#A5B4FC"/><rect x="15" y="28" width="11" height="3" rx="1.5" fill="#A5B4FC"/><path d="M31 27h18a6 6 0 0 1 6 6v10a6 6 0 0 1-6 6v6l-7-6H31a6 6 0 0 1-6-6V33a6 6 0 0 1 6-6z" fill="#34D399" stroke="#1E1B4B" stroke-width="2.5" stroke-linejoin="round"/><path d="M33 38.5l4.5 4.5 8.5-9" fill="none" stroke="#064E3B" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';

export const MOBILE_MANIFEST = JSON.stringify({
    name: 'Co-Review', short_name: 'Co-Review', description: 'Review threads, findings and decisions on the go.',
    start_url: '/m/', scope: '/m/', display: 'standalone', background_color: '#17162B', theme_color: '#1E1B4B',
    icons: [{ src: '/m/icon.svg', sizes: 'any', type: 'image/svg+xml', purpose: 'any maskable' }]
});

/** Pass-through worker: makes the page installable; no caching (the review is live). */
export const MOBILE_SERVICE_WORKER = "self.addEventListener('install', e => self.skipWaiting());"
    + "self.addEventListener('activate', e => e.waitUntil(self.clients.claim()));"
    + "self.addEventListener('fetch', e => e.respondWith(fetch(e.request)));";

export const MOBILE_PAGE = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="theme-color" content="#1E1B4B">
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-title" content="Co-Review">
<link rel="manifest" href="/m/manifest.webmanifest">
<link rel="icon" href="/m/icon.svg">
<link rel="apple-touch-icon" href="/m/icon.svg">
<title>Co-Review</title>
<style>
  :root { --bg: #ffffff; --fg: #1E1B4B; --muted: #5B5A7E; --line: #E4E5F2; --card: #F6F6FB; --accent: #4F46E5; --agent: #059669; --ok: #059669; --bad: #DC2626; --warn: #B45309; }
  @media (prefers-color-scheme: dark) { :root { --bg: #131226; --fg: #D6D8F0; --muted: #9C9BC4; --line: #2E2D57; --card: #1C1B36; --accent: #818CF8; --agent: #34D399; --ok: #34D399; --bad: #F87171; --warn: #FCD34D; } }
  * { box-sizing: border-box; }
  body { margin: 0; font: 15px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; background: var(--bg); color: var(--fg); padding-bottom: env(safe-area-inset-bottom); }
  header { position: sticky; top: 0; z-index: 2; background: var(--bg); border-bottom: 1px solid var(--line); padding: calc(10px + env(safe-area-inset-top)) 14px 10px; display: flex; align-items: center; gap: 10px; }
  header h1 { font-size: 17px; margin: 0; flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  button { font: inherit; border-radius: 8px; border: 1px solid var(--line); background: var(--card); color: var(--fg); padding: 7px 12px; }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button.link { border: none; background: none; color: var(--accent); padding: 4px 2px; }
  main { padding: 10px 12px 90px; display: flex; flex-direction: column; gap: 10px; }
  .muted { color: var(--muted); }
  .row { display: flex; align-items: center; gap: 8px; }
  .spacer { flex: 1; }
  .card { border: 1px solid var(--line); border-radius: 12px; padding: 10px 12px; background: var(--card); display: flex; flex-direction: column; gap: 8px; }
  .review { text-decoration: none; color: inherit; }
  .tabs { display: flex; gap: 6px; overflow-x: auto; }
  .tab { border-radius: 16px; padding: 4px 12px; white-space: nowrap; }
  .agent-dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--agent); margin-right: 6px; vertical-align: middle; }
  .tab.on { background: var(--accent); color: #fff; border-color: var(--accent); }
  .label { font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--accent); overflow-wrap: anywhere; }
  .badge { font-size: 11px; border: 1px solid var(--line); border-radius: 10px; padding: 0 7px; color: var(--muted); white-space: nowrap; }
  .badge.high { color: var(--bad); border-color: var(--bad); } .badge.medium { color: var(--warn); border-color: var(--warn); }
  .badge.proposed { color: var(--accent); border-color: var(--accent); } .badge.q { color: var(--agent); border-color: var(--agent); }
  pre { margin: 0; padding: 8px; border-radius: 8px; background: var(--bg); border-left: 3px solid var(--warn); overflow-x: auto; font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre; }
  .msg { display: flex; flex-direction: column; gap: 2px; }
  .msg.agent { border-left: 3px solid var(--agent); padding-left: 8px; }
  .msg.system { color: var(--muted); font-style: italic; }
  .who { font-weight: 600; font-size: 13px; } .msg.agent .who { color: var(--agent); }
  .body { white-space: pre-wrap; word-break: break-word; }
  .body code { font: 13px ui-monospace, Menlo, monospace; background: var(--bg); padding: 0 3px; border-radius: 4px; }
  textarea { width: 100%; font: inherit; border-radius: 10px; border: 1px solid var(--line); background: var(--bg); color: var(--fg); padding: 8px 10px; min-height: 70px; }
  .before { background: rgba(248, 81, 73, 0.12); border-left-color: var(--bad); } .after { background: rgba(46, 160, 67, 0.14); border-left-color: var(--ok); }
  .sheet { position: fixed; left: 0; right: 0; bottom: 0; z-index: 5; background: var(--bg); border-top: 1px solid var(--line); border-radius: 16px 16px 0 0; padding: 14px 14px calc(14px + env(safe-area-inset-bottom)); box-shadow: 0 -6px 30px rgba(0,0,0,.2); display: flex; flex-direction: column; gap: 10px; }
  .decisions { display: flex; gap: 6px; } .decisions button { flex: 1; } .decisions button.on { border-color: var(--accent); color: var(--accent); font-weight: 600; }
  .fab { position: fixed; right: 14px; bottom: calc(16px + env(safe-area-inset-bottom)); z-index: 3; border-radius: 22px; padding: 10px 18px; box-shadow: 0 4px 14px rgba(0,0,0,.2); }
  .empty { text-align: center; padding: 40px 10px; }
</style>
</head>
<body>
<header><button class="link" id="back" hidden>&#8249; Reviews</button><img src="/m/icon.svg" alt="" width="26" height="26" style="border-radius:6px"><h1 id="title">Co-Review</h1><span class="muted" id="agent"></span></header>
<main id="main"><div class="empty muted">Loading…</div></main>
<script>
(function () {
  if ('serviceWorker' in navigator) { navigator.serviceWorker.register('/m/sw.js', { scope: '/m/' }).catch(function () {}); }
  var main = document.getElementById('main'), title = document.getElementById('title'), back = document.getElementById('back'), agentEl = document.getElementById('agent');
  var state = { review: null, filter: 'open', since: 0, polling: 0, drafts: {} };
  function esc(s) { return String(s == null ? '' : s).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  function md(s) { return esc(s).replace(/\\x60([^\\x60]+)\\x60/g, '<code>$1</code>').replace(/\\*\\*([^*]+)\\*\\*/g, '<b>$1</b>'); }
  function api(p, body) { return fetch('/api/m' + p, body ? { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) } : {}).then(function (r) { return r.json(); }); }
  function el(html) { var d = document.createElement('div'); d.innerHTML = html.trim(); return d.firstChild; }
  function param(name) { return new URLSearchParams(location.search).get(name); }

  function showList() {
    state.review = null; back.hidden = true; title.textContent = 'Co-Review'; agentEl.textContent = '';
    api('/reviews').then(function (reviews) {
      main.innerHTML = '';
      if (!reviews.length) { main.innerHTML = '<div class="empty muted">No reviews yet.</div>'; return; }
      reviews.forEach(function (r) {
        var a = el('<a class="review card" href="?review=' + r.id + '"><div class="row"><b>' + esc(r.title) + '</b></div>'
          + '<div class="row muted"><span>' + esc(r.repository) + '</span><span class="spacer"></span><span>' + r.open + ' open' + (r.proposed ? ' · ' + r.proposed + ' proposed' : '') + '</span></div></a>');
        a.onclick = function (e) { e.preventDefault(); history.pushState({}, '', '?review=' + r.id); openReview(r.id); };
        main.appendChild(a);
      });
    });
  }

  function openReview(id) {
    back.hidden = false; state.since = 0; state.review = null; main.innerHTML = '<div class="empty muted">Loading…</div>';
    var token = ++state.polling;
    (function poll() {
      api('/reviews/' + id + '?since=' + state.since).then(function (review) {
        if (token !== state.polling) { return; }
        if (review.error) { main.innerHTML = '<div class="empty muted">' + esc(review.error) + '</div>'; return; }
        state.since = review.updatedAt; state.review = review; render();
        poll();
      }).catch(function () { if (token === state.polling) { setTimeout(poll, 3000); } });
    })();
  }

  function render() {
    var review = state.review; if (!review) { return; }
    // Keep what the reviewer is typing across live updates.
    main.querySelectorAll('textarea[data-thread]').forEach(function (t) { state.drafts[t.getAttribute('data-thread')] = t.value; });
    title.textContent = review.title; agentEl.innerHTML = review.agent ? '<span class="agent-dot"></span>' + esc(review.agent) : '';
    var counts = { open: 0, proposed: 0, resolved: 0 };
    review.threads.forEach(function (t) { counts[t.status] = (counts[t.status] || 0) + 1; });
    main.innerHTML = '';
    var tabs = el('<div class="tabs"></div>');
    [['open', 'Open ' + counts.open], ['proposed', 'Proposed ' + counts.proposed], ['resolved', 'Resolved ' + counts.resolved]].forEach(function (f) {
      if (f[0] === 'proposed' && !counts.proposed) { return; }
      var b = el('<button class="tab' + (state.filter === f[0] ? ' on' : '') + '">' + f[1] + '</button>');
      b.onclick = function () { state.filter = f[0]; render(); };
      tabs.appendChild(b);
    });
    main.appendChild(tabs);
    if (review.verdict) { main.appendChild(el('<div class="muted">Round ' + review.verdict.count + ': ' + esc(review.verdict.decision) + (review.verdict.summary ? ' — ' + esc(review.verdict.summary) : '') + '</div>')); }
    var shown = review.threads.filter(function (t) { return t.status === state.filter; });
    if (!shown.length) { main.appendChild(el('<div class="empty muted">No ' + state.filter + ' threads.</div>')); }
    shown.forEach(function (t) { main.appendChild(threadCard(review, t)); });
    var fab = el('<button class="primary fab">Submit review</button>');
    fab.onclick = function () { submitSheet(review); };
    main.appendChild(fab);
  }

  function threadCard(review, t) {
    var card = el('<div class="card"></div>');
    var head = '<div class="row"><b>#' + t.number + '</b><span class="label">' + esc(t.label) + '</span><span class="spacer"></span>'
      + (t.intent === 'question' ? '<span class="badge q">question</span>' : '')
      + (t.severity ? '<span class="badge ' + t.severity + '">' + t.severity + '</span>' : '')
      + (t.status === 'proposed' ? '<span class="badge proposed">proposed</span>' : '')
      + (t.agentState === 'working' ? '<span class="badge q">agent working…</span>' : '') + '</div>';
    card.appendChild(el(head));
    if (t.code) { card.appendChild(el('<pre>' + esc(t.code.split('\\n').slice(0, 12).join('\\n')) + '</pre>')); }
    if (t.proposal) {
      card.appendChild(el('<pre class="before">' + esc(t.proposal.before) + '</pre>'));
      card.appendChild(el('<pre class="after">' + esc(t.proposal.after) + '</pre>'));
      if (t.proposal.status === 'pending') {
        var pr = el('<div class="row"><span class="muted">Suggested edit</span><span class="spacer"></span><button>Reject</button><button class="primary">Accept edit</button></div>');
        pr.children[2].onclick = function () { api('/reviews/' + review.id + '/threads/' + t.id + '/proposal', { accept: false }); };
        pr.children[3].onclick = function () { api('/reviews/' + review.id + '/threads/' + t.id + '/proposal', { accept: true }); };
        card.appendChild(pr);
      } else { card.appendChild(el('<div class="muted">Suggestion ' + t.proposal.status + '</div>')); }
    }
    t.messages.forEach(function (m) {
      card.appendChild(el('<div class="msg ' + m.kind + '"><div class="row"><span class="who">' + esc(m.author) + '</span><span class="spacer"></span><span class="muted">'
        + new Date(m.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) + (m.status === 'streaming' ? ' · …' : '') + '</span></div><div class="body">' + md(m.body) + '</div></div>'));
    });
    var actions = el('<div class="row"></div>');
    function action(label, status, primary) {
      var b = el('<button' + (primary ? ' class="primary"' : '') + '>' + label + '</button>');
      b.onclick = function () { api('/reviews/' + review.id + '/threads/' + t.id + '/status', { status: status }); };
      actions.appendChild(b);
    }
    if (t.status === 'proposed') { action('Dismiss', 'resolved'); action('Accept', 'open', true); }
    else if (t.status === 'open') {
      var reply = el('<textarea data-thread="' + t.id + '" placeholder="Reply…" rows="2"></textarea>');
      reply.value = state.drafts[t.id] || '';
      card.appendChild(reply);
      var send = el('<button class="primary">Reply</button>');
      send.onclick = function () {
        var body = reply.value.trim(); if (!body) { return; }
        delete state.drafts[t.id]; reply.value = '';
        api('/reviews/' + review.id + '/threads/' + t.id + '/messages', { body: body });
      };
      actions.appendChild(el('<span class="spacer"></span>'));
      action('Resolve', 'resolved'); actions.appendChild(send);
    } else { action('Reopen', 'open'); }
    card.appendChild(actions);
    return card;
  }

  function submitSheet(review) {
    var decision = 'comment';
    var sheet = el('<div class="sheet"><b>Submit review to the agent</b><div class="decisions"><button data-d="approve">Approve</button><button data-d="request-changes">Changes</button><button data-d="comment" class="on">Comment</button></div>'
      + '<textarea placeholder="Message to the agent (optional)"></textarea><div class="row"><button class="link">Cancel</button><span class="spacer"></span><button class="primary">Submit</button></div></div>');
    sheet.querySelectorAll('[data-d]').forEach(function (b) {
      b.onclick = function () { decision = b.getAttribute('data-d'); sheet.querySelectorAll('[data-d]').forEach(function (x) { x.classList.toggle('on', x === b); }); };
    });
    sheet.querySelector('.link').onclick = function () { sheet.remove(); };
    sheet.querySelector('.row .primary').onclick = function () {
      api('/reviews/' + review.id + '/submit', { decision: decision, summary: sheet.querySelector('textarea').value.trim() }).then(function () { sheet.remove(); });
    };
    document.body.appendChild(sheet);
  }

  back.onclick = function () { state.polling++; history.pushState({}, '', '/m/'); showList(); };
  window.onpopstate = function () { var id = param('review'); if (id) { openReview(id); } else { state.polling++; showList(); } };
  var initial = param('review'); if (initial) { openReview(initial); } else { showList(); }
})();
</script>
</body>
</html>`;

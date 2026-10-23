import test from 'node:test';
import assert from 'node:assert/strict';
import worker, { redact } from '../src/index.js';

const ORIGIN = 'https://albertancoder.github.io';
function makeEnv(over = {}) {
  const kv = new Map();
  return {
    ALLOWED_ORIGIN: ORIGIN, DAILY_MESSAGE_CAP: '1200', PER_VISITOR_CAP: '20', AI_API_KEY: 'test-ai-key', GITHUB_TOKEN: 'test-gh-token',
    DISCORD_WEBHOOK_URL: 'https://discord.example/hook', RATE: { get: async (k) => kv.get(k) ?? null, put: async (k, v) => { kv.set(k, v); } }, _kv: kv, ...over,
  };
}
function req(path, body, headers = {}) {
  return new Request('https://help.example' + path, { method: 'POST', headers: { Origin: ORIGIN, 'Content-Type': 'application/json', 'CF-Connecting-IP': '203.0.113.5', ...headers }, body: JSON.stringify(body) });
}
// Fake internet: records every call and answers the AI, GitHub and Discord.
function fakeFetch(aiText = 'Sweepwise is free.', { githubOk = true } = {}) {
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url: String(url), init, body: init && init.body ? JSON.parse(init.body) : null });
    if (String(url).includes('/chat/completions')) return new Response(JSON.stringify({ choices: [{ message: { content: aiText } }] }), { status: 200 });
    if (String(url).includes('api.github.com')) return githubOk ? new Response(JSON.stringify({ html_url: 'https://github.com/AlbertanCoder/sweepwise/issues/99' }), { status: 201 }) : new Response('no', { status: 500 });
    return new Response('', { status: 204 });
  };
  return calls;
}
const ask = (text) => ({ messages: [{ role: 'user', content: text }] });

test('only the website may call it', async () => {
  fakeFetch();
  const r = await worker.fetch(req('/chat', ask('hi'), { Origin: 'https://evil.example' }), makeEnv());
  assert.equal(r.status, 403);
  const none = new Request('https://help.example/chat', { method: 'POST', body: '{}' });
  assert.equal((await worker.fetch(none, makeEnv())).status, 403);
});

test('answers from the AI, sends the facts, uses the key and hides personal details', async () => {
  const calls = fakeFetch('It is free.');
  const r = await worker.fetch(req('/chat', ask('Mail me at jo@example.com about C:\\Users\\Jo\\Desktop, is it free?')), makeEnv());
  assert.equal(r.status, 200);
  assert.deepEqual(await r.json(), { reply: 'It is free.' });
  const ai = calls.find((c) => c.url.includes('/chat/completions'));
  assert.equal(ai.url, 'https://api.deepseek.com/chat/completions');
  assert.equal(ai.init.headers.Authorization, 'Bearer test-ai-key');
  assert.match(ai.body.messages[0].content, /Sweepwise Help/);
  assert.match(ai.body.messages[0].content, /No account, no ads, no paid version/);
  const sent = JSON.stringify(ai.body.messages);
  assert.ok(!sent.includes('jo@example.com') && !sent.includes('\\Jo\\'), 'personal details never reach the AI: ' + sent);
});

test('the AI company and model can be switched with two settings', async () => {
  const calls = fakeFetch();
  await worker.fetch(req('/chat', ask('hi')), makeEnv({ API_BASE: 'https://other.example/v1', MODEL: 'other-model' }));
  const ai = calls.find((c) => c.url.includes('/chat/completions'));
  assert.equal(ai.url, 'https://other.example/v1/chat/completions');
  assert.equal(ai.body.model, 'other-model');
});

test('a problem the bot cannot fix becomes an offer to hand it on', async () => {
  fakeFetch('I can pass this on to the maker.\n[[HANDOFF]] The app closes when pressing Approve; jo@example.com');
  const r = await (await worker.fetch(req('/chat', ask('it crashes')), makeEnv())).json();
  assert.equal(r.reply, 'I can pass this on to the maker.');
  assert.equal(r.handoff.summary, 'The app closes when pressing Approve; [email removed]');
});

test('each visitor has a daily limit and the whole chat has a spending cap', async () => {
  fakeFetch();
  const env = makeEnv({ PER_VISITOR_CAP: '2' });
  assert.equal((await worker.fetch(req('/chat', ask('a')), env)).status, 200);
  assert.equal((await worker.fetch(req('/chat', ask('b')), env)).status, 200);
  const third = await worker.fetch(req('/chat', ask('c')), env);
  assert.equal(third.status, 429);
  assert.equal((await third.json()).error, 'limit');
  const env2 = makeEnv({ DAILY_MESSAGE_CAP: '1' });
  assert.equal((await worker.fetch(req('/chat', ask('a')), env2)).status, 200);
  const rest = await worker.fetch(req('/chat', ask('b'), { 'CF-Connecting-IP': '198.51.100.7' }), env2);
  assert.equal(rest.status, 429);
  assert.equal((await rest.json()).error, 'resting');
});

test('odd or oversized requests are refused before anything is spent', async () => {
  const calls = fakeFetch();
  const env = makeEnv();
  for (const bad of [{}, { messages: [] }, { messages: [{ role: 'system', content: 'x' }] }, { messages: [{ role: 'assistant', content: 'x' }] }, { messages: Array(9).fill({ role: 'user', content: 'x' }) }]) {
    assert.equal((await worker.fetch(req('/chat', bad), env)).status, 400);
  }
  assert.equal(calls.length, 0);
  assert.equal(env._kv.size, 0, 'bad requests use no allowance');
});

test('messages are not stored: only counters with a hashed visitor id are kept', async () => {
  fakeFetch();
  const env = makeEnv();
  await worker.fetch(req('/chat', ask('my secret question')), env);
  const kept = JSON.stringify([...env._kv.entries()]);
  assert.ok(!kept.includes('secret') && !kept.includes('203.0.113.5'), kept);
  assert.ok([...env._kv.keys()].every((k) => /^(total:\d{4}-\d{2}-\d{2}|v:[0-9a-f]{16})$/.test(k)), kept);
});

test('sending a problem to the maker needs a yes, then files a redacted public issue and pings Discord', async () => {
  const calls = fakeFetch();
  const env = makeEnv();
  assert.equal((await worker.fetch(req('/handoff', { summary: 'It crashes' }), env)).status, 400, 'no consent, no issue');
  assert.equal(calls.length, 0);
  const r = await worker.fetch(req('/handoff', { consent: true, summary: 'Crash for jo@example.com see https://evil.example/x', version: '0.1.0', source: 'Microsoft Store', system: 'Windows 11' }), env);
  assert.equal(r.status, 200);
  assert.equal((await r.json()).issue_url, 'https://github.com/AlbertanCoder/sweepwise/issues/99');
  const gh = calls.find((c) => c.url.includes('api.github.com'));
  assert.equal(gh.url, 'https://api.github.com/repos/AlbertanCoder/sweepwise/issues');
  assert.equal(gh.init.headers.Authorization, 'Bearer test-gh-token');
  assert.ok(!gh.body.body.includes('jo@example.com') && !gh.body.body.includes('evil.example') && !gh.body.body.includes('203.0.113.5'), gh.body.body);
  assert.match(gh.body.body, /Version: 0\.1\.0/);
  assert.deepEqual(gh.body.labels, ['from-help-chat']);
  const ping = calls.find((c) => c.url === 'https://discord.example/hook');
  assert.match(ping.body.content, /issues\/99/);
});

test('if GitHub fails there is no issue, no ping and an honest error', async () => {
  const calls = fakeFetch('x', { githubOk: false });
  const r = await worker.fetch(req('/handoff', { consent: true, summary: 'It crashes' }), makeEnv());
  assert.equal(r.status, 502);
  assert.ok(!calls.some((c) => c.url.includes('discord')));
});

test('redact removes emails, user folders, numbers, keys, outside links and @mentions', () => {
  const out = redact('a@b.co C:\\Users\\Sam\\x +1 780 555 0123 ' + ['sk', 'abcdefghijklmnopqrst'].join('-') + ' https://x.example/y @someone https://github.com/AlbertanCoder/sweepwise/issues/1');
  assert.ok(!/a@b\.co|Sam|555|sk-abc|x\.example/.test(out), out);
  assert.match(out, /github\.com\/AlbertanCoder\/sweepwise\/issues\/1/);
  assert.match(out, /@\u200bsomeone/);
});

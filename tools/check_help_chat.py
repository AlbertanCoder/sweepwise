"""Checks the website's help chat bubble (docs/help-chat.js) in a headless browser, with a pretend back end.

Serves docs/ locally, adds a pretend back-end address to the page, and answers /chat and /handoff itself, so no AI
company, GitHub or Discord is ever contacted. Run: python -I tools/check_help_chat.py   (exits 0 only if every check passes)"""
import http.server
import json
import pathlib
import re
import sys
import threading
import functools

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding='utf-8')
DOCS = pathlib.Path(__file__).resolve().parent.parent / 'docs'
FAKE = 'https://help.test'
fails = []


def check(cond, what):
    print(('PASS ' if cond else 'FAIL ') + what)
    if not cond:
        fails.append(what)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def main():
    handler = functools.partial(Quiet, directory=str(DOCS))
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    local = 'http://127.0.0.1:%d/' % server.server_address[1]
    posts, blocked, errors = [], [], []
    mode = {'chat': 'ok'}

    with sync_playwright() as p:
        browser = p.chromium.launch()

        def page(w, h, endpoint):
            ctx = browser.new_context(viewport={'width': w, 'height': h})

            def route(r):
                u = r.request.url
                if u.startswith(FAKE):
                    if r.request.method == 'OPTIONS':
                        return r.fulfill(status=204, headers={'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Headers': 'Content-Type', 'Access-Control-Allow-Methods': 'POST'})
                    body = json.loads(r.request.post_data or '{}')
                    posts.append((u.replace(FAKE, ''), body))
                    h = {'Access-Control-Allow-Origin': '*', 'Content-Type': 'application/json'}
                    if u.endswith('/chat'):
                        if mode['chat'] == 'resting':
                            return r.fulfill(status=429, headers=h, body=json.dumps({'error': 'resting'}))
                        if mode['chat'] == 'handoff':
                            return r.fulfill(status=200, headers=h, body=json.dumps({'reply': 'I can pass this on to the maker.', 'handoff': {'summary': 'The app closes when pressing Approve.'}}))
                        return r.fulfill(status=200, headers=h, body=json.dumps({'reply': 'Yes, every feature is free.'}))
                    return r.fulfill(status=200, headers=h, body=json.dumps({'issue_url': 'https://github.com/AlbertanCoder/sweepwise/issues/99'}))
                if u.startswith(local):
                    if u.rstrip('/') == local.rstrip('/') or u.endswith('index.html'):
                        resp = r.fetch()
                        html = re.sub(r'data-endpoint="[^"]*"', 'data-endpoint="%s"' % endpoint, resp.text())
                        return r.fulfill(response=resp, body=html)
                    return r.continue_()
                blocked.append(u)
                r.abort()
            ctx.route('**/*', route)
            pg = ctx.new_page()
            pg.on('pageerror', lambda e: errors.append(str(e)))
            pg.on('console', lambda m: errors.append(m.text) if m.type == 'error' and 'ERR_FAILED' not in m.text and 'status of 429' not in m.text else None)
            return ctx, pg

        # off by default: no back-end address means no bubble
        ctx, pg = page(1280, 800, '')
        pg.goto(local)
        check(pg.locator('.hc-open').count() == 0, 'without a back-end address the page shows no chat bubble')
        ctx.close()

        ctx, pg = page(1280, 800, FAKE)
        pg.goto(local)
        check(pg.is_visible('.hc-open') and pg.inner_text('.hc-open') == 'Ask Sweepwise Help', 'with an address the bubble is shown')
        pg.click('.hc-open')
        check(pg.is_visible('.hc-panel') and 'AI assistant' in pg.inner_text('.hc-note') and 'DeepSeek' in pg.inner_text('.hc-note') and 'personal details' in pg.inner_text('.hc-note'),
              'the open chat says it is an AI, names the AI company and warns about personal details')
        pg.fill('.hc-in', 'Is it free?')
        pg.press('.hc-in', 'Enter')
        pg.wait_for_selector('.hc-bot:has-text("every feature is free")')
        check(posts[-1][0] == '/chat' and posts[-1][1] == {'messages': [{'role': 'user', 'content': 'Is it free?'}]}, 'a question posts just the typed messages to /chat: %s' % (posts[-1],))
        pg.fill('.hc-in', 'And the Store?')
        pg.press('.hc-in', 'Enter')
        pg.wait_for_function("() => !document.querySelector('.hc-form button').disabled && document.querySelectorAll('.hc-bot').length >= 3")
        check([m['role'] for m in posts[-1][1]['messages']] == ['user', 'assistant', 'user'], 'the second question carries the earlier turns')

        mode['chat'] = 'handoff'
        pg.wait_for_function("() => !document.querySelector('.hc-form button').disabled")
        pg.fill('.hc-in', 'It crashes')
        pg.press('.hc-in', 'Enter')
        pg.wait_for_selector('.hc-card')
        card = pg.inner_text('.hc-card')
        check('posted publicly on GitHub' in card and 'The app closes when pressing Approve.' in card, 'a hand-off shows exactly what would be posted and where: %s' % card.replace('\n', ' | '))
        n = len(posts)
        pg.click('.hc-card .hc-btn:has-text("No thanks")')
        check(len(posts) == n and pg.locator('.hc-card').count() == 0, '"No thanks" sends nothing')
        pg.wait_for_function("() => !document.querySelector('.hc-form button').disabled")
        pg.fill('.hc-in', 'It crashes again')
        pg.press('.hc-in', 'Enter')
        pg.wait_for_selector('.hc-card')
        pg.click('.hc-card .hc-btn:has-text("Send it")')
        pg.wait_for_selector('.hc-bot a[href$="/issues/99"]')
        check(posts[-1][0] == '/handoff' and posts[-1][1].get('consent') is True and posts[-1][1].get('summary') == 'The app closes when pressing Approve.', '"Send it" posts consent and the shown summary: %s' % (posts[-1],))

        mode['chat'] = 'resting'
        pg.wait_for_function("() => !document.querySelector('.hc-form button').disabled")
        pg.fill('.hc-in', 'hello')
        pg.press('.hc-in', 'Enter')
        pg.wait_for_selector('.hc-bot:has-text("resting")')
        check(pg.locator('.hc-bot a[href="https://github.com/AlbertanCoder/sweepwise/issues/new"]').count() >= 1, 'when the chat is resting the person is pointed to Report a problem')
        pg.keyboard.press('Escape')
        check(not pg.is_visible('.hc-panel') and pg.is_visible('.hc-open'), 'Escape closes the chat')
        ctx.close()

        ctx, pg = page(400, 760, FAKE)
        pg.goto(local)
        pg.click('.hc-open')
        box = pg.eval_on_selector('.hc-panel', 'e => { const r = e.getBoundingClientRect(); return [r.left, r.top, r.right, r.bottom, innerWidth, innerHeight, document.documentElement.scrollWidth]; }')
        check(box[0] >= 0 and box[2] <= box[4] and box[3] <= box[5] and box[6] == box[4], 'on a phone-width screen the chat fits and the page does not scroll sideways: %s' % box)
        ctx.close()
        browser.close()

    check(not errors, 'no script errors: %s' % errors[:3])
    check(not blocked, 'nothing was requested outside this PC and the pretend back end: %s' % blocked[:3])
    print('%d failed' % len(fails))
    sys.exit(1 if fails else 0)


main()

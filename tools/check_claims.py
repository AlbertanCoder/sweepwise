"""Keeps Sweepwise's promises honest: checks a claims list against the files it points at.

    python tools/check_claims.py tools/claims.json

Each entry is one sentence Sweepwise uses to say what it does, with the code that makes it true:
    {"id", "source", "text", "status", "proof": [{"file", "contains"}], "tests": [...], "why"}
- "proven" or "fixed": the source still says "text", every proof file still contains its "contains", and every test
  named in "tests" still exists as a Rust #[test] function.
- "removed": an overpromise that was taken out; the source must no longer say "text".
Anything else (for example "open") fails. Paths are absolute or relative to the repo root. Spaces and line breaks are
compared loosely, so re-wrapping a paragraph doesn't break the check.
"""
import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
ledger = pathlib.Path(sys.argv[1])
items = json.loads(ledger.read_text(encoding='utf-8'))


def squash(s):
    return re.sub(r'\s+', ' ', s).strip()


_cache = {}


def text_of(path):
    p = pathlib.Path(path)
    p = p if p.is_absolute() else root / p
    if p not in _cache:
        _cache[p] = squash(p.read_text(encoding='utf-8')) if p.exists() else None
    return _cache[p]


tests = set(re.findall(r'\bfn\s+([a-z0-9_]+)\s*\(', '\n'.join(
    f.read_text(encoding='utf-8') for f in (root / 'src-tauri' / 'src').rglob('*.rs'))))
problems, counts = [], {}
seen = set()
for c in items:
    cid, status = c.get('id', '?'), c.get('status')
    counts[status] = counts.get(status, 0) + 1
    if cid in seen:
        problems.append('%s: id used twice' % cid)
    seen.add(cid)
    src = text_of(c['source'])
    if src is None:
        problems.append('%s: source %s is missing' % (cid, c['source']))
        continue
    said = squash(c['text']) in src
    if status == 'removed':
        if said:
            problems.append('%s: removed overpromise is still in %s: %r' % (cid, c['source'], c['text'][:80]))
        continue
    if status not in ('proven', 'fixed'):
        problems.append('%s: status %r is not settled' % (cid, status))
        continue
    if not said:
        problems.append('%s: %s no longer says %r (update the claim or its proof)' % (cid, c['source'], c['text'][:80]))
    if not c.get('proof') and not c.get('tests'):
        problems.append('%s: no proof given' % cid)
    for pr in c.get('proof', []):
        body = text_of(pr['file'])
        if body is None:
            problems.append('%s: proof file %s is missing' % (cid, pr['file']))
        elif squash(pr['contains']) not in body:
            problems.append('%s: proof %r not found in %s' % (cid, pr['contains'][:60], pr['file']))
    for t in c.get('tests', []):
        if t not in tests:
            problems.append('%s: test %s not found' % (cid, t))

for p in problems:
    print('PROBLEM ' + p)
print('%s: %d claims (%s), %d problems' % (ledger.name, len(items),
      ', '.join('%s %d' % kv for kv in sorted(counts.items(), key=lambda kv: str(kv[0]))), len(problems)))
sys.exit(1 if problems else 0)

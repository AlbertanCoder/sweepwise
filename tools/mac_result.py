# Writes tools/mac/RESULT.json on a Mac: test counts, the built program's self-test, a summary of the read-only
# real-system tour, and the DMG next to it. Used by the Mac GitHub job.
#   python3 tools/mac_result.py <test.log> <tour.json> <out dir> <built program>
import datetime
import glob
import json
import os
import re
import subprocess
import sys
import tempfile

log_path, tour_path, out_dir, program = sys.argv[1:5]

text = open(log_path, encoding='utf-8', errors='replace').read() if os.path.isfile(log_path) else ''
results = re.findall(r'^test result: \w+\. (\d+) passed; (\d+) failed', text, re.M)

st = {}
if os.path.isfile(program):
    with tempfile.TemporaryDirectory() as data:
        try:
            run = subprocess.run([program, '--self-test'], capture_output=True, text=True, timeout=120, env={**os.environ, 'SWEEPWISE_DATA': data})
            lines = run.stdout.strip().splitlines()
            st = json.loads(lines[-1]) if lines else {}
        except (OSError, ValueError, subprocess.TimeoutExpired):
            st = {}

tour = {}
try:
    raw = open(tour_path, encoding='utf-8', errors='replace').read()
    tour = json.loads(raw[raw.index('{'):])
except (OSError, ValueError):
    tour = {}
sec = tour.get('security', {})
status = tour.get('status', {})

try:
    mac_version = subprocess.run(['sw_vers', '-productVersion'], capture_output=True, text=True, timeout=30).stdout.strip()
except OSError:
    mac_version = ''

dmgs = sorted(glob.glob(os.path.join(out_dir, '*.dmg')), key=os.path.getmtime)
r = {
    'platform': st.get('platform', 'unknown'),
    'self_test_ok': bool(st.get('ok')),
    'version': st.get('version'),
    'features': st.get('features', []),
    'tests_passed': sum(int(p) for p, _ in results),
    'tests_failed': sum(int(f) for _, f in results),
    'dmg': os.path.basename(dmgs[-1]) if dmgs else None,
    'macos': mac_version,
    'tour': {
        'score': status.get('score'),
        'reasons': status.get('reasons', []),
        'protection': len(sec.get('protection', [])),
        'posture': len(sec.get('posture', [])),
        'protection_items': [[p.get('name'), p.get('ok')] for p in sec.get('protection', [])],
        'posture_items': [[p.get('name'), p.get('level')] for p in sec.get('posture', [])],
        'updates': sec.get('updates'),
        'startup': len(tour.get('startup', [])),
        'apps': tour.get('apps'),
        'app_updates': tour.get('updates'),
        'deep_plan': tour.get('deep_plan', {}).get('would_clean'),
    },
    'when': datetime.datetime.now().isoformat(timespec='seconds'),
}
with open(os.path.join(out_dir, 'RESULT.json'), 'w', encoding='utf-8') as f:
    json.dump(r, f, indent=1)
print(json.dumps({k: v for k, v in r.items() if k != 'features'}, indent=1))

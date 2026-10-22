# Writes tools/linux/RESULT.json from a Linux test log, the built program's self-test and the packages next to it.
# Used by tools/linux_check.sh and by the GitHub checks.
#   python3 tools/linux_result.py <test.log> <out dir> <built program> [test exit] [build exit]
import datetime
import glob
import json
import os
import re
import subprocess
import sys
import tempfile

log_path, out_dir, program = sys.argv[1:4]
test_exit = int(sys.argv[4]) if len(sys.argv) > 4 else 0
build_exit = int(sys.argv[5]) if len(sys.argv) > 5 else 0

text = open(log_path, encoding='utf-8', errors='replace').read() if os.path.isfile(log_path) else ''
results = re.findall(r'^test result: \w+\. (\d+) passed; (\d+) failed', text, re.M)
passed = sum(int(p) for p, _ in results)
failed = sum(int(f) for _, f in results)
if test_exit and not failed:
    failed = 1

st = {}
if os.path.isfile(program):
    with tempfile.TemporaryDirectory() as data:
        try:
            run = subprocess.run([program, '--self-test'], capture_output=True, text=True, timeout=120, env={**os.environ, 'SWEEPWISE_DATA': data})
            lines = run.stdout.strip().splitlines()
            st = json.loads(lines[-1]) if lines else {}
        except (OSError, ValueError, subprocess.TimeoutExpired):
            st = {}


def newest(pattern):
    found = sorted(glob.glob(os.path.join(out_dir, pattern)), key=os.path.getmtime)
    return os.path.basename(found[-1]) if found else None


distro = ''
try:
    with open('/etc/os-release', encoding='utf-8') as f:
        for line in f:
            if line.startswith('PRETTY_NAME='):
                distro = line.split('=', 1)[1].strip().strip('"')
except OSError:
    pass

r = {
    'platform': st.get('platform', 'unknown'),
    'self_test_ok': bool(st.get('ok')),
    'version': st.get('version'),
    'features': st.get('features', []),
    'tests_passed': passed,
    'tests_failed': failed,
    'test_exit': test_exit,
    'build_exit': build_exit,
    'deb': newest('*.deb'),
    'appimage': newest('*.AppImage'),
    'rpm': newest('*.rpm'),
    'distro': distro,
    'when': datetime.datetime.now().isoformat(timespec='seconds'),
}
with open(os.path.join(out_dir, 'RESULT.json'), 'w', encoding='utf-8') as f:
    json.dump(r, f, indent=1)
print(json.dumps({k: v for k, v in r.items() if k != 'features'}))

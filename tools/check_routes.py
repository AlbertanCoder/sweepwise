# Checks that every request the Sweepwise screens make has an answer in the Rust engine and every job the screens
# start exists. (Email used to be banned from the public app by PUBLIC_SPEC answer 9; answer 40 brought it back,
# so email requests are now checked like every other request.)
# Usage: python -I tools/check_routes.py <sweepwise folder> [--email]   (--email: the email screens must be there too)
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
html = (root / 'src' / 'index.html').read_text(encoding='utf-8')
rs = (root / 'src-tauri' / 'src' / 'engine' / 'mod.rs').read_text(encoding='utf-8')

used = set(re.findall(r"api\('(/api/[a-z_/]+)", html))
answered = set(re.findall(r'"(/api/[a-z_/]+)"', rs))
modes = set(re.findall(r"run\('([a-z_]+)'", html)) | set(re.findall(r'data-run="([a-z_]+)"', html))
jobs = set(re.findall(r'"([a-z_]+)"\s*(?:\||=>)', rs[rs.index('fn start_job'):rs.index('fn route')]))
# The AI Organizer ('organize') arrives in Stage P3; until then the engine refuses it with a plain message.
later = {'organize'}

problems = []
missing = sorted(used - answered)
if missing:
    problems.append('screens ask for requests the engine does not answer: ' + ', '.join(missing))
missing_jobs = sorted(modes - jobs - later)
if missing_jobs:
    problems.append('screens start jobs the engine does not know: ' + ', '.join(missing_jobs))
email = sorted(u for u in used if 'email' in u)
if '--email' in sys.argv[2:] and not email:
    problems.append('the email screens are missing (PUBLIC_SPEC answer 40)')

print(f'{len(used)} screen requests, all answered: {not missing}; {len(modes)} jobs ({", ".join(sorted(modes))}); email requests: {len(email)}')
if problems:
    print('\n'.join(problems))
    sys.exit(1)

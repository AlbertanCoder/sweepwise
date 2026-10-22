"""Makes the Windows installer's terms page (src-tauri/installer/terms.rtf) from the app's terms (src/i18n/terms.en.html),
so the installer, the app and the signing page always show the same words.

    python tools/make_terms_rtf.py           writes the RTF
    python tools/make_terms_rtf.py --check   exits 1 if the RTF is out of date
"""
import html
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
src = root / 'src' / 'i18n' / 'terms.en.html'
out = root / 'src-tauri' / 'installer' / 'terms.rtf'


def rtf_text(fragment):
    """Inline HTML (only <b> and <span>) to RTF text."""
    parts = []
    for piece in re.split(r'(</?b>)', fragment):
        if piece == '<b>':
            parts.append(r'{\b ')
        elif piece == '</b>':
            parts.append('}')
        else:
            text = html.unescape(re.sub(r'</?span[^>]*>', '', piece))
            if not text.isascii():
                raise SystemExit('terms.en.html must stay plain ASCII for the installer: %r' % text)
            parts.append(text.replace('\\', '\\\\').replace('{', '\\{').replace('}', '\\}'))
    return ''.join(parts)


def build():
    body = src.read_text(encoding='utf-8')
    lines = [r'{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss\fcharset0 Segoe UI;}}',
             r'\viewkind4\uc1\f0\fs18']
    for tag, attrs, inner in re.findall(r'<(h3|p)([^>]*)>(.*?)</\1>', body, re.S):
        inner = ' '.join(inner.split())
        cls = re.search(r'class="([^"]+)"', attrs)
        cls = cls.group(1) if cls else ''
        if tag == 'h3':
            lines.append(r'\pard\sa80{\b\fs22 ' + rtf_text(inner) + r'}\par')
        elif cls == 'ver':
            lines.append(r'\pard\sa200{\i ' + rtf_text(inner) + r'}\par')
        elif cls == 'sub':
            lines.append(r'\pard\li360\sa100 ' + rtf_text(inner) + r'\par')
        else:
            lines.append(r'\pard\sa140 ' + rtf_text(inner) + r'\par')
    lines.append('}')
    return '\r\n'.join(lines) + '\r\n'


rtf = build()
if '--check' in sys.argv:
    if not out.exists() or out.read_bytes().decode('ascii') != rtf:
        print('terms.rtf is out of date: run python tools/make_terms_rtf.py')
        sys.exit(1)
    print('terms.rtf matches terms.en.html')
else:
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(rtf.encode('ascii'))
    print('wrote', out.relative_to(root), len(rtf), 'bytes')

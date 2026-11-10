"""Regenerates fixtures/latex/warmup/*.tex from the engine's own LaTeX template.

Compiling these files once (without --only-cached) puts every package and font file that create_pdf can use
into the Tectonic cache: Latin/Greek/Cyrillic with Noto, CJK with Fandol/Harano Aji/Un fonts, all four classes,
tables, lists, images, math, links, strike-through (ulem) and the table of contents.

    .local/document-tools-20261018/engine-venv/Scripts/python.exe crates/toolbox/tests/document_engine/make_warmup.py
"""
import pathlib
import shutil
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import builders  # noqa: E402
from harness import Stage  # noqa: E402

BODY = ('# Überschrift Ελληνικά Привет\nText mit **fett**, *kursiv*, ~~durch~~, `code`, [Link](https://example.org) '
        'und \\(x_i^2\\).\n\n1. eins\n   - zwei\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n> Zitat\n\n```\ncode\n```\n\n'
        '$$\\begin{pmatrix}1&2\\\\3&4\\end{pmatrix}\\sum_{i=1}^n \\frac{a}{b}$$\n\n![Bild](input:0)\n\n---\n')
DOCUMENTS = {
    'warmup-article-de.tex': {'title': 'Warm-up', 'author': 'Engine', 'toc': True, 'markdown': BODY},
    'warmup-report-en.tex': {'title': 'Warm-up', 'lang': 'en', 'document_class': 'report', 'font': 'sans',
                             'paper': 'letter', 'markdown': BODY},
    'warmup-scrartcl.tex': {'document_class': 'scrartcl', 'font_size': 10, 'markdown': BODY},
    'warmup-scrreprt.tex': {'document_class': 'scrreprt', 'font_size': 12, 'paper': 'a5', 'markdown': BODY},
    'warmup-cjk-zh.tex': {'title': '中文', 'markdown': '中文文本 **粗体**'},
    'warmup-cjk-ja.tex': {'title': '日本語', 'markdown': 'ひらがなとカタカナと漢字 **太字**'},
    'warmup-cjk-ko.tex': {'title': '한국어', 'markdown': '한국어 텍스트 **굵게**'},
}


def main():
    target = HERE / 'fixtures' / 'latex' / 'warmup'
    target.mkdir(parents=True, exist_ok=True)
    tmp = tempfile.mkdtemp()
    try:
        for name, spec in DOCUMENTS.items():
            stage = Stage(tmp)
            stage.add('image.png', builders.image(120, 80))
            code, receipt, stderr = stage.run('document_write', 'create_pdf', spec, output='w.pdf',
                                              phase='latex_prepare')
            if code != 0:
                raise SystemExit('%s: %s\n%s' % (name, receipt, stderr))
            (target / name).write_bytes(stage.read('work/latex/main.tex'))
            for asset in receipt['result']['assets']:
                (target / 'assets').mkdir(exist_ok=True)
                (target / 'assets' / pathlib.Path(asset).name).write_bytes(stage.read(asset))
            print('wrote', target / name)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == '__main__':
    main()

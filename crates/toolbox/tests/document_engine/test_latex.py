"""LaTeX templating (latex_prepare), log parsing and QA (latex_finalize), optional real Tectonic compile."""
import io
import os
import pathlib
import re
import shutil
import subprocess
import unittest

import builders
from harness import FIXTURES, REPO, EngineCase

LOGS = FIXTURES / 'latex'


def body_of(tex):
    return tex.split('\\begin{document}', 1)[1].split('\\end{document}', 1)[0]


class LatexPrepareTests(EngineCase):
    def prepare(self, spec, stage=None, code=None):
        stage = stage or self.stage()
        if code:
            return self.failed(stage, code, 'document_write', 'create_pdf', spec, output='doc.pdf',
                               phase='latex_prepare')
        receipt = self.ok(stage, 'document_write', 'create_pdf', spec, output='doc.pdf', phase='latex_prepare')
        self.assertEqual(receipt['outputs'], [])
        self.assertEqual(receipt['result']['main'], 'work/latex/main.tex')
        return receipt, stage.read('work/latex/main.tex').decode('utf-8'), stage

    def test_every_special_character_is_escaped(self):
        text = 'a\\b{c}$d&e#f^g_h~i%j"k<l>m|n[o]p`q--r\'\'s\u00a0t\tu'
        _, tex, _ = self.prepare({'blocks': [{'type': 'paragraph', 'text': text}]})
        expected = ('a\\textbackslash{}b\\{c\\}\\$d\\&e\\#f\\textasciicircum{}g\\_h\\textasciitilde{}i\\%j'
                    '\\textquotedbl{}k\\textless{}l\\textgreater{}m\\textbar{}n{[}o{]}p\\textasciigrave{}q-{}-r\'{}\'s~t u')
        self.assertEqual(body_of(tex).strip(), expected)

    def test_escaping_in_titles_lists_tables_and_markdown(self):
        _, tex, _ = self.prepare({
            'title': 'Q&A 100%', 'author': 'A_B', 'date': '1. Jan', 'lang': 'en', 'toc': True,
            'document_class': 'scrreprt', 'paper': 'letter', 'font_size': 12, 'font': 'sans',
            'markdown': '# Kosten & $\n\n- [x] erledigt\n- 50% \\(x_1\\)\n\n| a#b | c |\n|---|---:|\n| 1 | 2 |\n\n'
                        '```\n\\input{x} %\n```\n\n> Zitat }\n\n$$\\frac{a}{b}$$'})
        self.assertIn('\\title{Q\\&A 100\\%}', tex)
        self.assertIn('\\author{A\\_B}', tex)
        self.assertIn('\\date{1. Jan}', tex)
        self.assertIn('\\documentclass[12pt,letterpaper]{scrreprt}', tex)
        self.assertIn('\\usepackage[ngerman,english]{babel}', tex)
        self.assertIn('\\renewcommand{\\familydefault}{\\sfdefault}', tex)
        body = body_of(tex)
        self.assertIn('\\chapter{Kosten \\& \\$}', body)
        self.assertIn('\\item{} ☒ erledigt', body)
        self.assertIn('\\item{} 50\\% \\(x_1\\)', body)
        self.assertIn('\\textbf{a\\#b} & \\textbf{c} \\\\', body)
        self.assertIn('>{\\raggedleft\\arraybackslash}', tex)
        self.assertIn('\\mbox{}\\textbackslash{}input\\{x\\}\\ \\%\\par', body)
        self.assertIn('\\begin{quote}\nZitat \\}\n\\end{quote}', body)
        self.assertIn('\\[\n\\frac{a}{b}\n\\]', body)
        self.assertIn('\\tableofcontents', body)

    def test_blocks(self):
        receipt, tex, stage = self.prepare({'blocks': [
            {'type': 'heading', 'text': 'Eins\nZwei', 'level': 2},
            {'type': 'paragraph', 'runs': [{'text': 'f', 'bold': True}, {'text': 'k', 'italic': True},
                                           {'text': 'u', 'underline': True}, {'text': 's', 'strike': True},
                                           {'text': 'c', 'code': True}, {'text': 'r', 'color': 'FF0000'},
                                           {'text': 'l', 'link': 'https://x.org/a_b#c%20d'},
                                           {'math': 'e^{i\\pi}'}, {'text': 'g', 'size': 14},
                                           {'text': 'h', 'highlight': 'yellow'}], 'align': 'center'},
            {'type': 'paragraph', 'text': 'Zeile 1\nZeile 2'},
            {'type': 'list', 'ordered': True, 'start': 3, 'items': [{'text': 'a', 'items': [{'text': 'b', 'items': [
                {'text': 'c', 'items': [{'text': 'd', 'items': ['e']}]}]}]}]},
            {'type': 'table', 'rows': [['x', {'text': 'y', 'colspan': 2}], ['1', '2', '3']], 'caption': 'T',
             'widths_cm': [2, 3, 4]},
            {'type': 'page_break'}, {'type': 'horizontal_rule'}, {'type': 'toc', 'levels': 2},
            {'type': 'math', 'latex': '\\begin{pmatrix}1&2\\\\3&4\\end{pmatrix}'}]})
        body = body_of(tex)
        self.assertIn('\\subsection{Eins Zwei}', body)
        self.assertIn('\\begin{center}\n\\textbf{f}\\emph{k}\\uline{u}\\sout{s}\\texttt{c}'
                      '\\textcolor[HTML]{FF0000}{r}\\href{https://x.org/a\\_b\\#c\\%20d}{l}\\(e^{i\\pi}\\)'
                      '{\\fontsize{14.0pt}{16.8pt}\\selectfont g}h\n\\end{center}', body)
        self.assertIn('Zeile 1\\newline Zeile 2', body)
        self.assertIn('\\begin{enumerate}[start=3]', body)
        self.assertEqual(body.count('\\begin{enumerate}'), 4)
        self.assertIn('\\multicolumn{2}{l}{\\textbf{y}}', body)
        self.assertIn('>{\\raggedright\\arraybackslash}p{4.00cm}', body)
        self.assertIn('\\caption{T}\\\\', body)
        self.assertIn('\\setcounter{tocdepth}{2}', body)
        self.assertTrue(any('flattened' in d['message'] for d in receipt['diagnostics']))
        self.assertTrue(any('highlight' in d['message'] for d in receipt['diagnostics']))

    def test_math_is_filtered(self):
        refused = ['\\input{/etc/passwd}', '\\immediate\\write18{calc}', '\\catcode`\\@=11', '^^5cinput{x}',
                   '\\begin{filecontents}{x}y\\end{filecontents}', '\\csname input\\endcsname', '\\def\\x{1}',
                   'a % comment', '\\UseName{input}', '\\makeatletter', '\\XeTeXpicfile x', '\\openin1=x',
                   '\\special{pdf:x}', '$x$', 'x\\]', '{', '\\@input', '\\let\\a\\b', '\\expandafter',
                   '\\begin{document}', '\\scantokens{x}', '\\verb|x|', '\\newcommand\\x{y}', '\\usepackage{x}',
                   '\\jobname', '\\pdffiledump', '\\read16 to \\x', '\\uppercase{\\x}']
        stage = self.stage()
        for latex in refused:
            receipt = self.prepare({'blocks': [{'type': 'math', 'latex': latex}]}, stage=stage, code='invalid_spec')
            self.assertEqual(receipt['error']['path'], 'spec.blocks[0].latex', latex)
        allowed = ['\\frac{a}{b}', '\\sum_{i=1}^n i^2', '\\operatorname{sin} x', '\\text{if } x \\geq 0',
                   '\\mathbb{R}', '\\left(\\frac12\\right)', '\\begin{cases}1 & x>0\\\\0\\end{cases}',
                   '\\begin{aligned}a&=b\\\\c&=d\\end{aligned}', '\\alpha\\beta\\varepsilon\\Rightarrow\\infty',
                   '100\\%', '\\{a\\}']
        for latex in allowed:
            self.prepare({'blocks': [{'type': 'math', 'latex': latex}]})

    def test_cjk_font_selection(self):
        for text, font in (('中文', 'FandolSong-Regular.otf'), ('日本語のテキスト', 'HaranoAjiMincho-Regular.otf'),
                           ('한국어', 'UnBatang.ttf')):
            _, tex, _ = self.prepare({'markdown': text})
            self.assertIn('\\usepackage{xeCJK}', tex)
            self.assertIn('\\setCJKmainfont{%s}' % font, tex)
            self.assertIn('\\IfFileExists{xeCJK.sty}', tex)
        _, tex, _ = self.prepare({'markdown': 'Grüße Привет Γειά'})
        self.assertNotIn('xeCJK', tex)
        self.assertIn('\\IfFontExistsTF{NotoSerif-Regular.ttf}', tex)
        self.assertIn('\\tracinglostchars=2', tex)

    def test_images_are_staged_without_exif(self):
        stage = self.stage()
        stage.add('photo.jpg', builders.jpeg_with_exif())
        stage.add('plot.pdf', builders.sample_pdf(1))
        receipt, tex, stage = self.prepare({'markdown': '![Foto](input:0)\n\n![Plot](input:1)'}, stage=stage)
        self.assertEqual(receipt['result']['assets'], ['work/latex/assets/image-0.jpg',
                                                       'work/latex/assets/image-1.pdf'])
        self.assertIn('\\includegraphics[width=1.06cm,height=0.45\\textheight,keepaspectratio]'
                      '{assets/image-0.jpg}', tex)  # 40 px at 96 dpi
        self.assertIn('\\includegraphics[width=16.00cm,height=0.45\\textheight,keepaspectratio]'
                      '{assets/image-1.pdf}', tex)
        self.assertIn('\\caption{Foto}', tex)
        from PIL import Image
        image = Image.open(io.BytesIO(stage.read('work/latex/assets/image-0.jpg')))
        self.assertEqual(dict(image.getexif()), {})
        self.assertEqual(image.size, (40, 60))  # orientation 6 was applied

    def test_requires_content_and_pdf_output(self):
        stage = self.stage()
        self.failed(stage, 'invalid_spec', 'document_write', 'create_pdf', {'title': 'x'}, output='a.pdf',
                    phase='latex_prepare')
        self.failed(stage, 'invalid_spec', 'document_write', 'create_pdf', {'markdown': 'x'}, output='a.docx',
                    phase='latex_prepare')


class LatexFinalizeTests(EngineCase):
    def finalize(self, log_name=None, pdf=None, failed=False, operation='compile_latex', extra=None):
        stage = self.stage()
        stage.add('main.tex', b'\\documentclass{article}', name='in/tex/main.tex')
        stage.write('in/tex/chapters/intro.tex', b'x')
        stage.write('in/tex/sections/part.tex', b'x')
        options = {'compile_failed': failed, 'pdf': 'work/latex-out/main.pdf', 'log': 'work/latex-out/main.log'}
        if log_name:
            stage.write('work/latex-out/main.log', (LOGS / log_name).read_bytes())
        if pdf:
            stage.write('work/latex-out/main.pdf', pdf)
        options.update(extra or {})
        return stage, options

    def test_errors_with_file_and_line(self):
        stage, options = self.finalize('undefined.log', failed=True)
        receipt = self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                              phase='latex_finalize', options=options)
        errors = [d for d in receipt['diagnostics'] if d['severity'] == 'error']
        self.assertEqual(len(errors), 2, receipt['diagnostics'])
        self.assertEqual((errors[0]['file'], errors[0]['line']), ('chapters/intro.tex', 3))
        self.assertTrue(errors[0]['message'].startswith('chapters/intro.tex:3: Undefined control sequence \\foo'))
        self.assertEqual((errors[1]['file'], errors[1]['line']), ('main.tex', 17))
        self.assertIn('Missing $ inserted', errors[1]['message'])
        self.assertIn('near: x_', errors[1]['message'])
        self.assertTrue(receipt['error']['message'].startswith('chapters/intro.tex:3:'))

    def test_missing_package(self):
        stage, options = self.finalize('missing-package.log', failed=True)
        receipt = self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                              phase='latex_finalize', options=options)
        errors = [d for d in receipt['diagnostics'] if d['severity'] == 'error']
        self.assertEqual(len(errors), 1, errors)
        self.assertEqual((errors[0]['file'], errors[0]['line']), ('main.tex', 4))
        self.assertIn("File `minted.sty' not found", errors[0]['message'])
        self.assertIn('offline cache', errors[0]['message'])

    def test_extensionless_input_log_uses_the_actual_companion_file(self):
        # Actual Tectonic output for \input{chapters/intro} omits the suffix.
        stage, options = self.finalize(failed=True)
        stage.write('work/latex-out/main.log', '(main.tex\n(article.cls (size10.clo))\n'
                    ' (chapters/intro\n! Undefined control sequence.\nl.1 \\undefinedProjectCommand\n\n')
        receipt = self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                              phase='latex_finalize', options=options)
        error = receipt['diagnostics'][0]
        self.assertEqual((error['file'], error['line']), ('chapters/intro.tex', 1))

    def test_log_source_resolution_prefers_the_full_path_over_a_shared_basename(self):
        for suffix in ('', '.tex'):
            stage, options = self.finalize(failed=True)
            stage.write('in/tex/intro.tex', b'outer')
            stage.write('work/latex-out/main.log', '(main.tex\n'
                        '(C:/private/source/chapters/intro%s\n! Undefined control sequence.\nl.7 \\bad\n\n' % suffix)
            receipt = self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                                  phase='latex_finalize', options=options)
            self.assertEqual(receipt['diagnostics'][0]['file'], 'chapters/intro.tex')

    def test_success_with_warnings_and_qa(self):
        stage, options = self.finalize('warnings.log', pdf=builders.sample_pdf(2), extra={'preview': True})
        receipt = self.ok(stage, 'document_write', 'compile_latex', output='main.pdf', phase='latex_finalize',
                          options=options)
        qa = receipt['qa']
        self.assertEqual(qa['pages'], 2)
        self.assertEqual(qa['overfull_boxes'], 2)
        self.assertEqual(qa['underfull_boxes'], 1)
        self.assertEqual(qa['missing_characters'], ['中 U+4E2D', '文 U+6587'])
        self.assertEqual(qa['empty_pages'], [])
        messages = [d['message'] for d in receipt['diagnostics']]
        self.assertIn('main.tex:20: Overfull box: 15.1pt too wide', messages)
        self.assertIn('sections/part.tex:7: Overfull box: 2.5pt too wide', messages)
        self.assertTrue(any(m.startswith('Missing characters in font NotoSerif: 中 (U+4E2D), 文 (U+6587)')
                            for m in messages))
        self.assertTrue(any(m.startswith('main.tex:9: hyperref: Token not allowed') for m in messages))
        self.assertTrue(any(m.startswith("main.tex:14: Reference `fig:x'") for m in messages))
        self.assertFalse(any('Font shape' in m for m in messages))
        self.assertEqual(receipt['outputs'][0]['name'], 'out/main.pdf')
        self.assertEqual(len(receipt['previews']), 2)
        self.assertEqual(receipt['result']['missing_characters'], 2)

    def test_diagnostics_are_capped(self):
        log = '(./main.tex\n' + ''.join('! Undefined control sequence.\nl.%d \\bad%d\n\n' % (i, i) for i in range(100))
        stage, options = self.finalize(failed=True)
        stage.write('work/latex-out/main.log', log)
        receipt = self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                              phase='latex_finalize', options=options)
        self.assertEqual(len(receipt['diagnostics']), 40)

    def test_failure_without_log_uses_stderr(self):
        stage, options = self.finalize(failed=True, extra={'stderr': 'work/latex-out/stderr.txt'})
        stage.write('work/latex-out/stderr.txt', 'note: x\nerror: the file "main.tex" could not be found\n'
                                                 'error: halted on potentially-recoverable error as specified\n')
        receipt = self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                              phase='latex_finalize', options=options)
        self.assertEqual(receipt['error']['message'], 'the file "main.tex" could not be found')

    def test_missing_pdf_is_an_error(self):
        stage, options = self.finalize('warnings.log')
        self.failed(stage, 'latex_error', 'document_write', 'compile_latex', output='main.pdf',
                    phase='latex_finalize', options=options)

    def test_empty_page_detection(self):
        from reportlab.pdfgen import canvas
        buffer = io.BytesIO()
        drawing = canvas.Canvas(buffer)
        drawing.drawString(72, 700, 'Text')
        drawing.showPage()
        drawing.showPage()
        drawing.save()
        stage, options = self.finalize('warnings.log', pdf=buffer.getvalue())
        receipt = self.ok(stage, 'document_write', 'compile_latex', output='main.pdf', phase='latex_finalize',
                          options=options)
        self.assertEqual(receipt['qa']['empty_pages'], [2])


def find_tectonic():
    """COWORK_TECTONIC/COWORK_TECTONIC_CACHE, or the binary the Setup session noted in TECTONIC-SPIKE.md."""
    binary, cache = os.environ.get('COWORK_TECTONIC'), os.environ.get('COWORK_TECTONIC_CACHE')
    if binary and cache:
        return pathlib.Path(binary), pathlib.Path(cache)
    notes = REPO / '.local' / 'document-tools-20261018' / 'TECTONIC-SPIKE.md'
    if not notes.is_file():
        return None
    match = re.search(r'Scratch evidence: `([^`]+)`', notes.read_text(encoding='utf-8', errors='replace'))
    if not match:
        return None
    root = pathlib.Path(match.group(1).rstrip('\\/'))
    for candidate in (root / 'bin' / 'msvc' / 'tectonic.exe', root / 'bin' / 'tectonic.exe', root / 'bin' / 'tectonic'):
        if candidate.is_file() and (root / 'cache').is_dir():
            return candidate, root / 'cache'
    return None


class RealCompileTests(EngineCase):
    def test_template_compiles_with_tectonic(self):
        found = find_tectonic()
        if not found:
            self.skipTest('no Tectonic binary/cache (set COWORK_TECTONIC and COWORK_TECTONIC_CACHE)')
        binary, cache = found
        stage = self.stage()
        stage.add('photo.png', builders.image(120, 80))
        self.ok(stage, 'document_write', 'create_pdf', {
            'title': 'Probe', 'author': 'Engine', 'markdown': '# Überschrift\nText mit **fett**, `code`, 50 % & '
            'Sonderzeichen: \\ { } $ # ^ _ ~ " < > | [ ].\n\n- a\n- b\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\n'
            '$$\\sum_{i=1}^n i$$\n\n![Bild](input:0)'}, output='probe.pdf', phase='latex_prepare')
        out = stage.root / 'work' / 'latex-out'
        out.mkdir(parents=True)
        env = dict(os.environ, TECTONIC_CACHE_DIR=str(cache))
        process = subprocess.run([str(binary), '-X', 'compile', '--untrusted', '--only-cached', '--keep-logs',
                                  '--outdir', str(out), str(stage.root / 'work' / 'latex' / 'main.tex')],
                                 cwd=str(stage.root / 'work' / 'latex'), env=env, capture_output=True, timeout=300)
        stage.write('work/latex-out/stderr.txt', process.stderr)
        code, receipt, _ = stage.run('document_write', 'create_pdf', output='probe.pdf', phase='latex_finalize',
                                     options={'compile_failed': process.returncode != 0,
                                              'pdf': 'work/latex-out/main.pdf', 'log': 'work/latex-out/main.log',
                                              'stderr': 'work/latex-out/stderr.txt'})
        if not receipt['ok'] and re.search(r"File `[^']+' not found", receipt['error']['message']):
            self.skipTest('the Tectonic cache at %s lacks a file the template needs: %s' % (
                cache, receipt['error']['message']))
        self.assertTrue(receipt['ok'], receipt)
        self.assertGreaterEqual(receipt['qa']['pages'], 1)
        self.assertEqual(receipt['qa']['missing_characters'], [], receipt['diagnostics'])

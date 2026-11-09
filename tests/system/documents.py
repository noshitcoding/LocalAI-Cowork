"""Real HTTP document actions, durable review/drafts and publication recovery.

All files, configuration and database fault triggers belong to a disposable daemon.
Requires a Python with the document wheels; never uses a model or user data.
"""
import argparse
import contextlib
import csv
import hashlib
import io
import json
import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, default=pathlib.Path(
        'target/debug/cowork-daemon.exe' if os.name == 'nt' else 'target/debug/cowork-daemon'))
    parser.add_argument('--python', type=pathlib.Path, default=pathlib.Path(sys.executable))
    parser.add_argument('--toolbox-config', type=pathlib.Path,
        help='Require real Office qualification with LibreOffice from this configuration')
    parser.add_argument('--browser', action='store_true', help='Require the built UI and a private Playwright Chromium')
    parser.add_argument('--office-only', action='store_true', help='Run only the real Office cases for targeted diagnosis')
    parser.add_argument('--latex', action='store_true', help='Require isolated TeX using explicitly configured COWORK_TECTONIC and COWORK_TECTONIC_CACHE copies')
    parser.add_argument('--tex-config', type=pathlib.Path, help='Require isolated TeX from a private runtime_install.py toolbox.json and its sibling tool-runtime cache')
    args = parser.parse_args()
    if args.tex_config:
        args.latex = True
    if args.office_only and not args.toolbox_config:
        parser.error('--office-only requires --toolbox-config')
    binary, python = args.binary.resolve(), args.python.resolve()
    assert binary.is_file() and python.is_file()
    programs = {'python': str(python)}
    if args.toolbox_config:
        selected = json.loads(args.toolbox_config.read_text(encoding='utf-8'))['programs']['libreoffice']
        office = pathlib.Path(selected)
        assert office.is_absolute() and office.is_file(), 'Qualification needs an actual LibreOffice executable'
        # Only inherit the selected executable, never providers, folders or user data.
        programs['libreoffice'] = str(office)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='cowork-document-http-') as temporary:
        data = pathlib.Path(temporary)
        if os.name == 'nt':
            # Some workstations give inherited Users rights to TEMP. Scope ACL changes
            # strictly to this freshly created disposable data directory.
            options = {'creationflags': subprocess.CREATE_NO_WINDOW}
            identity = subprocess.check_output(['whoami', '/user', '/fo', 'csv', '/nh'], text=True, **options)
            sid = next(csv.reader(identity.strip().splitlines()))[1]
            assert sid.startswith('S-1-5-')
            subprocess.run(['icacls', str(data), '/inheritance:r', '/grant:r',
                f'*{sid}:(OI)(CI)F', '*S-1-5-18:(OI)(CI)F', '*S-1-5-32-544:(OI)(CI)F'],
                check=True, stdout=subprocess.DEVNULL, **options)
            subprocess.run(['icacls', str(data), '/setowner', '*' + sid],
                check=True, stdout=subprocess.DEVNULL, **options)
        if args.latex:
            if args.tex_config:
                configuration = args.tex_config.resolve()
                tex = pathlib.Path(json.loads(configuration.read_text(encoding='utf-8'))['programs']['tectonic'])
                assert tex.is_absolute()
                cache = configuration.parent / 'tool-runtime/tectonic-cache'
            else:
                tex = pathlib.Path(os.environ['COWORK_TECTONIC']).resolve()
                cache = pathlib.Path(os.environ['COWORK_TECTONIC_CACHE']).resolve()
            assert tex.is_file() and cache.is_dir(), 'Require an actual binary and warmed cache'
            # Confinement changes ACLs: use only disposable copies, never the selected runtime.
            runtime = data / 'tool-runtime'
            private_tex = runtime / 'tectonic-qualification' / tex.name
            private_tex.parent.mkdir(parents=True)
            shutil.copy2(tex, private_tex)
            shutil.copytree(cache, runtime / 'tectonic-cache')
            (runtime / 'tectonic-warm.json').write_text(json.dumps({'format': 'cowork-tectonic-warm/1',
                'tectonic_sha256': hashlib.sha256(private_tex.read_bytes()).hexdigest(),
                'fixture': 'explicit warmed cache copy; actual compilation required below'}), encoding='utf-8')
            programs['tectonic'] = str(private_tex)
        (data / 'toolbox.json').write_text(json.dumps({'programs': programs}), encoding='utf-8')
        env = {**os.environ, 'COWORK_DATA_DIR': str(data), 'COWORK_PORT': str(port), 'RUST_LOG': 'error'}
        for key in ('COWORK_DATABASE_URL', 'COWORK_WEB_DIR', 'COWORK_ENABLE_TEST_PROVIDER',
                    'COWORK_PUBLIC_ORIGIN', 'COWORK_DOCUMENT_RUNTIME_DIR'):
            env.pop(key, None)
        if args.browser:
            web = pathlib.Path(__file__).resolve().parents[2] / 'apps/web/dist'
            assert (web / 'index.html').is_file(), 'Build the current web UI first'
            env['COWORK_WEB_DIR'] = str(web)
        process, token = None, ''

        def call(method, path, body=None, authenticated=True, raw=False):
            headers = {'Content-Type': 'application/json'}
            if authenticated:
                headers['Authorization'] = 'Bearer ' + token
            request = urllib.request.Request(f'http://127.0.0.1:{port}/api/v2' + path,
                data=None if body is None else json.dumps(body).encode(), method=method, headers=headers)
            with urllib.request.urlopen(request, timeout=20) as response:
                policy = response.headers.get('Content-Security-Policy', '')
                assert "img-src 'self' data: blob:" in policy and "frame-src 'self' blob:" in policy, policy
                assert "object-src 'none'" in policy and "script-src 'self'" in policy, policy
                content = response.read()
                if raw:
                    assert response.headers['X-Cowork-Sha256'] == hashlib.sha256(content).hexdigest()
                    return content
                return json.loads(content)

        def rejected(method, path, body, status, **kwargs):
            try:
                call(method, path, body, **kwargs)
            except urllib.error.HTTPError as error:
                assert error.code == status, (path, error.code, error.read())
            else:
                raise AssertionError('Expected rejection: ' + path)

        def until(probe, predicate, label, seconds=40):
            deadline, value = time.monotonic() + seconds, None
            while time.monotonic() < deadline:
                value = probe()
                if predicate(value):
                    return value
                time.sleep(.05)
            raise AssertionError((label, value))

        def start():
            nonlocal process, token
            options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
            process = subprocess.Popen([str(binary)], env=env, stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE, **options)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError(process.stderr.read().decode(errors='replace'))
                try:
                    token = (data / 'daemon.token').read_text().strip()
                    call('GET', '/health')
                    break
                except (FileNotFoundError, PermissionError, urllib.error.URLError, TimeoutError):
                    time.sleep(.05)
            else:
                raise AssertionError('Daemon start timeout')
            until(lambda: call('GET', '/toolbox'), lambda value: value['documents_available'],
                'document runtime', seconds=120 if args.toolbox_config else 40)
            if args.toolbox_config:
                until(lambda: call('GET', '/toolbox'), lambda value: value['office_convert_available'],
                    'required LibreOffice conversion probe', seconds=120)
            if args.latex:
                until(lambda: call('GET', '/toolbox'), lambda value: value['latex_available'],
                    'required isolated LaTeX probe', seconds=120)

        def kill():
            nonlocal process
            if process:
                process.kill()
                process.wait(timeout=10)
                process.stderr.close()
                process = None

        def sql(statement):
            with contextlib.closing(sqlite3.connect(data / 'cowork-v2.sqlite3')) as db, db:
                db.execute(statement)

        def request(tool, key, mode='autonomous'):
            return {'version': 2, 'base_revision': call('GET', f'/orders/{order}')['revision'],
                'idempotency_key': key, 'access_mode': mode, 'tool': {'id': 'ui', **tool}}

        def prepare(body):
            return call('POST', f'/orders/{order}/document-actions/prepare', body)

        def commit(preparation):
            return call('POST', f"/orders/{order}/document-actions/{preparation['id']}/commit")

        def attention(run):
            return until(lambda: call('GET', f'/runs/{run}'), lambda value:
                value['run']['state'] in ('completed', 'failed', 'interrupted', 'canceled', 'expired') or
                value['run']['state'] == 'waiting_approval', 'document run', seconds=150 if args.toolbox_config or args.latex else 40)

        def complete(run):
            value = attention(run)
            assert value['run']['state'] == 'completed', {
                'run_id': run, 'state': value['run']['state'], 'error_code': value['run'].get('error_code'),
                'tool_results': [event['payload'].get('result') for event in call('GET', f'/runs/{run}/events?after=0')
                    if event['kind'] == 'tool_completed']}
            assert not value['run'].get('model_attempts', []) and value['run']['spec'].get('provider') is None
            assert not any(event['kind'].startswith('model_') for event in call('GET', f'/runs/{run}/events?after=0'))
            return value

        def create_tool(extension, path=None):
            spec = {'format': extension, 'markdown': '# Bericht\n\nUmlaute äöü und Text.'}
            if extension == 'xlsx':
                spec = {'format': extension, 'sheets': [{'rows': [['Name', 'Wert'], ['Nord', 3]]}]}
            elif extension == 'pptx':
                spec = {'format': extension, 'slides': [{'title': 'Bericht', 'text': 'Umlaute äöü'}]}
            elif extension == 'pdf':
                spec.pop('format')
            return {'name': 'document_write', 'arguments': {'operation': 'create_pdf' if extension == 'pdf' else 'create',
                'output': {'path': path or 'report.' + extension}, 'spec': spec}}

        def tool_result(run):
            events = call('GET', f'/runs/{run}/events?after=0')
            return next(event['payload']['result'] for event in reversed(events) if event['kind'] == 'tool_completed')

        def qualify_office():
            import openpyxl
            import xlsxwriter
            from pypdf import PdfReader

            # Values are deliberately stale and manual calculation is enabled.
            # A plain round trip must fail these assertions.
            buffer = io.BytesIO()
            workbook = xlsxwriter.Workbook(buffer, {'in_memory': True})
            workbook.set_calc_mode('manual')
            sheet = workbook.add_worksheet('Results')
            sheet.write_formula('A1', '=2*3', None, 999)
            sheet.write_formula('A2', "='Inputs'!A1*2", None, 999)
            sheet.write_formula('A3', '=IF(1=1,"","text")', None, 'stale')
            sheet.write_formula('A4', '=1/0', None, 999)
            sheet.write_string('B1', 'Umlaute äöü')
            workbook.add_worksheet('Inputs').write_number('A1', 7)
            workbook.close()
            source = buffer.getvalue()
            files = data / 'file-broker/workspaces' / order / 'files'
            (files / 'stale.xlsx').write_bytes(source)
            conversion = {'name': 'document_write', 'arguments': {'operation': 'convert',
                'inputs': [{'folder_id': folder, 'path': 'stale.xlsx'}],
                'output': {'path': 'calculated.xlsx'}, 'spec': {'recalculate': True}}}
            preparation = prepare(request(conversion, 'real-office-recalculation'))
            run = commit(preparation)['id']
            complete(run)
            output = call('GET', f'/orders/{order}/workspace/download?path=calculated.xlsx', raw=True)
            book = openpyxl.load_workbook(io.BytesIO(output), data_only=True)
            assert book['Results']['A1'].value == 6
            assert book['Results']['A2'].value == 14
            assert book['Results']['A3'].value in ('', None)
            assert book['Results']['A4'].value == '#DIV/0!'
            assert book['Results']['B1'].value == 'Umlaute äöü'
            book.close()
            report = tool_result(run)['engine']['qa']['calculation']
            assert report == {'status': 'errors', 'engine': 'libreoffice', 'formula_cells': 4,
                'missing_values': 0, 'error_values': 1, 'input_caches_cleared': 4}, report
            assert (files / 'stale.xlsx').read_bytes() == source
            assert commit(preparation)['id'] == run
            for extension in ('docx', 'pptx', 'xlsx'):
                preparation = prepare(request({'name': 'document_write', 'arguments': {'operation': 'convert',
                    'inputs': [{'folder_id': folder, 'path': 'report.' + extension}],
                    'output': {'path': 'converted-' + extension + '.pdf'}, 'spec': {}, 'preview': True}},
                    'real-office-pdf-' + extension))
                run = commit(preparation)['id']
                complete(run)
                content = call('GET', f'/orders/{order}/workspace/download?path=converted-{extension}.pdf', raw=True)
                pdf = PdfReader(io.BytesIO(content))
                assert len(pdf.pages) >= 1
                text = '\n'.join(page.extract_text() or '' for page in pdf.pages)
                assert ('Nord' if extension == 'xlsx' else 'Bericht') in text
                result = tool_result(run)
                assert result['engine']['previews']['count'] >= 1

        def qualify_browser():
            from PIL import Image, ImageDraw
            from playwright.sync_api import sync_playwright

            files = data / 'file-broker/workspaces' / order / 'files'
            image = Image.new('RGBA', (120, 80), 'white')
            ImageDraw.Draw(image).rectangle((0, 0, 59, 79), fill='red')
            image.save(files / 'ui-source.png')
            source = (files / 'ui-source.png').read_bytes()
            (files / 'paper/chapters').mkdir(parents=True)
            (files / 'paper/main.tex').write_text('\\documentclass{article}\n\\begin{document}\n\\input{chapters/intro}\n\\end{document}\n', encoding='utf-8')
            (files / 'paper/alternative.tex').write_text('\\documentclass{article}\n\\begin{document}Alternative\\end{document}\n', encoding='utf-8')
            (files / 'paper/chapters/intro.tex').write_text('Projekttext mit Umlauten äöü\nZweite Zeile\n', encoding='utf-8')
            view = {'state': {'language': 'de', 'theme': 'light', 'project': project['id'], 'order': order}, 'version': 0}
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                try:
                    context = browser.new_context(viewport={'width': 1280, 'height': 900})
                    page = context.new_page()
                    page.set_default_timeout(30000)
                    # Only this disposable origin receives the disposable test token.
                    page.goto(f'http://127.0.0.1:{port}/')
                    page.evaluate('''({token, base, view}) => {
                        sessionStorage.setItem('cowork-v2-token', token);
                        sessionStorage.setItem('cowork-v2-endpoint', base);
                        sessionStorage.setItem('cowork-v2-connection-kind', 'server');
                        localStorage.setItem('localai-cowork-v2-view', JSON.stringify(view));
                    }''', {'token': token, 'base': f'http://127.0.0.1:{port}', 'view': view})
                    page.reload()

                    def open_workbench():
                        page.get_by_role('button', name='Weitere Optionen', exact=True).click()
                        page.get_by_role('button', name='Dokumente Dateien und Office-Dokumente dieses Chats ansehen und bearbeiten', exact=True).click()

                    def decoded(name, width):
                        page.wait_for_function('''({name, width}) => Array.from(document.images).some(
                            image => image.alt === name && image.complete && image.naturalWidth === width)
                        ''', arg={'name': name, 'width': width})

                    def start_from_ui():
                        with page.expect_response(lambda response: response.request.method == 'POST' and response.url.endswith('/commit')) as committed:
                            page.get_by_role('button', name='Ausführung starten', exact=True).click()
                        record = committed.value.json()
                        assert 'id' in record, record
                        run = record['id']
                        value = attention(run)
                        assert value['run']['state'] == 'waiting_approval', {'state': value['run']['state'], 'error': value['run'].get('error_code')}
                        page.get_by_role('button', name='Erlauben', exact=True).click()
                        return run

                    open_workbench()
                    page.get_by_role('button', name='ui-source.png', exact=True).click()
                    decoded('ui-source.png', 120)
                    editor = page.locator('.image-pipeline')
                    editor.get_by_role('spinbutton', name='Breite', exact=True).fill('60')
                    editor.get_by_role('button', name='Schritt übernehmen', exact=True).click()
                    editor.get_by_role('combobox', name='Schritt hinzufügen', exact=True).select_option('rotate')
                    editor.get_by_role('button', name='Schritt übernehmen', exact=True).click()
                    assert editor.locator('ol > li').count() == 2
                    editor.get_by_role('button', name='Rückgängig', exact=True).click()
                    assert editor.locator('ol > li').count() == 1
                    editor.get_by_role('button', name='Wiederholen', exact=True).click()
                    editor.get_by_role('button', name='Schritt nach oben 2', exact=True).click()
                    assert editor.locator('ol > li > strong').first.inner_text() == 'Drehen'
                    editor.get_by_role('button', name='Schritt nach unten 1', exact=True).click()
                    page.get_by_role('button', name='Entwurf speichern', exact=True).click()
                    page.get_by_role('status').filter(has_text='Entwurf gespeichert · 1').wait_for()
                    page.reload()
                    open_workbench()
                    page.get_by_role('button', name='Entwurf laden', exact=True).click()
                    page.get_by_role('button', name='Öffnen', exact=True).click()
                    decoded('ui-source.png', 120)
                    assert editor.locator('ol > li').count() == 2
                    editor.get_by_role('button', name='Bildänderungen prüfen', exact=True).click()
                    page.get_by_role('button', name='Ausführung starten', exact=True).click()
                    page.get_by_role('button', name='Erlauben', exact=True).click()
                    generated = page.get_by_role('button', name='Ergebnis öffnen: ui-source-edited.png', exact=True)
                    generated.wait_for(timeout=60000)
                    generated.click()
                    decoded('ui-source-edited.png', 40)
                    assert page.get_by_role('alert').count() == 0
                    page.set_viewport_size({'width': 390, 'height': 844})
                    assert editor.evaluate('(element) => element.scrollWidth <= element.clientWidth + 2')
                    with Image.open(files / 'ui-source-edited.png') as result:
                        assert result.size == (40, 60)
                    assert (files / 'ui-source.png').read_bytes() == source
                    page.get_by_role('button', name='Nächste Aktion', exact=True).click()
                    project_editor = page.locator('.latex-project')
                    project_editor.get_by_role('textbox', name='Projektdateien (ein Pfad pro Zeile)', exact=True).fill('paper/main.tex\npaper/alternative.tex\npaper/chapters/intro.tex')
                    project_editor.get_by_role('button', name='Projektdateien erfassen', exact=True).click()
                    try:
                        project_editor.get_by_role('status').filter(has_text='Dateiversionen erfasst · 3/16').wait_for()
                    except Exception:
                        raise AssertionError(project_editor.inner_text()) from None
                    project_editor.get_by_role('combobox', name='Hauptdatei', exact=True).select_option('paper/alternative.tex')
                    project_editor.get_by_role('button', name='paper/chapters/intro.tex', exact=True).click()
                    assert project_editor.get_by_role('textbox', name='Projektquelltext', exact=True).input_value() == 'Projekttext mit Umlauten äöü\nZweite Zeile\n'
                    page.get_by_role('button', name='Entwurf speichern', exact=True).click()
                    page.get_by_role('status').filter(has_text='Entwurf gespeichert · 2').wait_for()
                    page.reload()
                    open_workbench()
                    page.get_by_role('button', name='Entwurf laden', exact=True).click()
                    assert project_editor.get_by_role('combobox', name='Hauptdatei', exact=True).input_value() == 'paper/alternative.tex'
                    project_editor.get_by_role('button', name='paper/chapters/intro.tex', exact=True).click()
                    project_editor.get_by_role('textbox', name='Projektquelltext', exact=True).wait_for()
                    if args.latex:
                        project_editor.get_by_role('button', name='Projektkompilierung prüfen', exact=True).click()
                        complete(start_from_ui())
                        generated_pdf = page.get_by_role('button', name='Ergebnis öffnen: compiled.pdf', exact=True)
                        generated_pdf.wait_for(timeout=150000)
                        assert (files / 'compiled.pdf').read_bytes().startswith(b'%PDF-')
                        page.get_by_role('button', name='Nächste Aktion', exact=True).click()
                    # A changed source must not be silently substituted for a saved version.
                    changed_source = '\\undefinedProjectCommand\n' if args.latex else 'Changed source'
                    (files / 'paper/chapters/intro.tex').write_text(changed_source, encoding='utf-8')
                    project_editor.get_by_role('button', name='paper/chapters/intro.tex', exact=True).click()
                    project_editor.get_by_role('alert').wait_for()
                    assert changed_source not in project_editor.get_by_role('textbox', name='Projektquelltext', exact=True).input_value()
                    if args.latex:
                        project_editor.get_by_role('textbox', name='Neue Projekt-PDF', exact=True).fill('broken-ui.pdf')
                        project_editor.get_by_role('button', name='Projektdateien erfassen', exact=True).click()
                        project_editor.get_by_role('status').filter(has_text='Dateiversionen erfasst · 3/16').wait_for()
                        project_editor.get_by_role('combobox', name='Hauptdatei', exact=True).select_option('paper/main.tex')
                        project_editor.get_by_role('button', name='Projektkompilierung prüfen', exact=True).click()
                        broken_run = start_from_ui()
                        value = attention(broken_run)
                        assert value['run']['state'] == 'failed', value['run']['state']
                        diagnostics = tool_result(broken_run).get('diagnostics', [])
                        page.locator('.document-diagnostics').wait_for()
                        location = page.get_by_role('button', name='chapters/intro.tex:1', exact=True)
                        assert location.count() == 1, {'diagnostics': diagnostics, 'ui': page.locator('.document-diagnostics').inner_text()}
                        location.click()
                        source_view = project_editor.get_by_role('textbox', name='Projektquelltext', exact=True)
                        assert source_view.input_value() == changed_source
                        assert source_view.evaluate('(element) => element.selectionStart') == 0
                        assert not (files / 'broken-ui.pdf').exists()
                    assert project_editor.evaluate('(element) => element.scrollWidth <= element.clientWidth + 2')
                finally:
                    browser.close()

        def qualify_latex():
            from PIL import Image
            from pypdf import PdfReader

            files = data / 'file-broker/workspaces' / order / 'files'
            (files / 'tex/chapters').mkdir(parents=True)
            main = files / 'tex/main.tex'
            chapter = files / 'tex/chapters/intro.tex'
            logo = files / 'tex/logo.png'
            main.write_text(r'''\documentclass{article}
\usepackage{graphicx}
\begin{document}
\section{Report}\label{sec:report}
\input{chapters/intro}
\includegraphics[width=20pt]{logo.png}
\end{document}
''', encoding='utf-8')
            chapter.write_text('Qualified project source. See Section~\\ref{sec:report}.\n', encoding='utf-8')
            Image.new('RGB', (30, 20), 'blue').save(logo)

            def compilation(path, key):
                inputs = [{'folder_id': folder, 'path': name, 'expected_sha256': hashlib.sha256((files / name).read_bytes()).hexdigest()}
                    for name in ('tex/main.tex', 'tex/chapters/intro.tex', 'tex/logo.png')]
                return prepare(request({'name': 'document_write', 'arguments': {'operation': 'compile_latex',
                    'inputs': inputs, 'output': {'path': path}, 'preview': True}}, key))

            prepared = compilation('project.pdf', 'tex-project')
            # The captured source tree and review remain bound across process restart.
            kill(); start()
            run = commit(prepared)['id']
            complete(run)
            content = call('GET', f'/orders/{order}/workspace/download?path=project.pdf', raw=True)
            text = '\n'.join(page.extract_text() or '' for page in PdfReader(io.BytesIO(content)).pages)
            assert 'Qualified project source' in text and '??' not in text
            assert tool_result(run)['engine']['previews']['count'] >= 1
            assert commit(prepared)['id'] == run
            stale = compilation('stale-project.pdf', 'tex-stale-project')
            chapter.write_text('\\undefinedProjectCommand\n', encoding='utf-8')
            rejected('POST', f"/orders/{order}/document-actions/{stale['id']}/commit", None, 409)
            broken = compilation('broken-project.pdf', 'tex-broken-project')
            run = commit(broken)['id']
            value = attention(run)
            assert value['run']['state'] == 'failed', value['run']['state']
            # Failed engine receipts are flattened into the runtime tool error.
            result = tool_result(run)
            assert result['error'] and any(item.get('file') == 'chapters/intro.tex' and item.get('line') == 1
                for item in result.get('diagnostics', [])), result
            assert not (files / 'broken-project.pdf').exists()
            chapter.write_text('Corrected project source.\n', encoding='utf-8')
            run = commit(compilation('corrected-project.pdf', 'tex-corrected-project'))['id']
            complete(run)
            corrected = call('GET', f'/orders/{order}/workspace/download?path=corrected-project.pdf', raw=True)
            assert 'Corrected project source' in '\n'.join(page.extract_text() or '' for page in PdfReader(io.BytesIO(corrected)).pages)
            main.write_text('\\documentclass{article}\n\\usepackage{coworkmissingqualification}\n\\begin{document}Missing package\\end{document}\n', encoding='utf-8')
            run = commit(compilation('missing-package.pdf', 'tex-missing-package'))['id']
            assert attention(run)['run']['state'] == 'failed'
            result = tool_result(run)
            assert result['error'] == 'latex_error' and 'not found' in result['message'], result
            assert not (files / 'missing-package.pdf').exists()

        try:
            start()
            assert call('GET', '/providers') == []
            project = call('POST', '/projects', {'name': 'Disposable document HTTP test'})
            order = call('POST', '/orders', {'project_id': project['id'], 'title': 'Document actions'})['id']
            other = call('POST', '/orders', {'project_id': project['id'], 'title': 'Other order'})['id']
            call('GET', f'/orders/{order}/workspace')
            if args.office_only:
                folder = call('GET', f'/orders/{order}/workspace')['folder']['id']
                for extension in ('docx', 'xlsx', 'pptx'):
                    complete(commit(prepare(request(create_tool(extension), 'create-' + extension)))['id'])
                qualify_office()
                print(json.dumps({'result': 'passed', 'real_libreoffice': True, 'model_used': False, 'scope': 'office-only'}))
                return
            draft = {'id': 'source', 'base_revision': 0, 'idempotency_key': 'draft-1',
                'content': {'format': 'tex', 'body': 'Original draft'}}
            first = call('PUT', f'/orders/{order}/document-drafts', draft)
            rejected('GET', f'/orders/{order}/document-drafts/source', None, 401, authenticated=False)
            rejected('GET', f'/orders/{other}/document-drafts/source', None, 404)
            body = request(create_tool('docx'), 'create-docx', 'confirm_mutations')
            rejected('POST', f'/orders/{order}/document-actions/prepare', dict(body, version=999), 400)
            preparation = prepare(body)
            kill(); start()
            assert prepare(body)['id'] == preparation['id']
            assert call('GET', f'/orders/{order}/document-actions/by-key/create-docx')['fingerprint'] == preparation['fingerprint']
            rejected('GET', f"/orders/{other}/document-actions/{preparation['id']}", None, 404)
            assert call('PUT', f'/orders/{order}/document-drafts', draft)['snapshot']['id'] == first['snapshot']['id']
            conflict = call('PUT', f'/orders/{order}/document-drafts', dict(draft,
                idempotency_key='draft-conflict', content={'body': 'Conflicting copy'}))
            assert not conflict['saved'] and conflict['snapshot']['conflict']
            assert conflict['snapshot']['content']['body'] == 'Conflicting copy'
            assert conflict['draft']['content'] == draft['content']
            assert len(call('GET', f'/orders/{order}/document-drafts/source/snapshots')) == 2
            run = commit(preparation)['id']
            assert attention(run)['run']['state'] == 'waiting_approval'
            kill(); start()
            value = attention(run)
            approval = next(a for a in value['approvals'] if a['decision'] is None)
            call('POST', f"/runs/{run}/approvals/{approval['id']}", {'fingerprint': approval['fingerprint'], 'allow': True})
            value = complete(run)
            assert len(value['file_operations']) == 1 and value['file_operations'][0]['kind'] == 'generate'
            assert commit(preparation)['id'] == run
            content = call('GET', f'/orders/{order}/workspace/download?path=report.docx', raw=True)
            assert content.startswith(b'PK')
            for extension in ('xlsx', 'pptx', 'pdf'):
                value = complete(commit(prepare(request(create_tool(extension), 'create-' + extension)))['id'])
                content = call('GET', f'/orders/{order}/workspace/download?path=report.{extension}', raw=True)
                assert content.startswith(b'%PDF' if extension == 'pdf' else b'PK')
            folder = call('GET', f'/orders/{order}/workspace')['folder']['id']
            reading = request({'name': 'document_read', 'arguments': {'operation': 'text', 'folder_id': folder,
                'path': 'report.pdf'}}, 'read-pdf', 'read_only')
            read = prepare(reading)
            assert len(read['tool']['arguments']['expected_sha256']) == 64
            assert not complete(commit(read)['id']).get('file_operations', [])
            if args.toolbox_config:
                qualify_office()
            if args.browser:
                qualify_browser()
            if args.latex:
                qualify_latex()
            rejected('POST', f'/orders/{order}/document-actions/prepare',
                request(create_tool('docx', 'forbidden.docx'), 'readonly-write', 'read_only'), 403)
            rejected('POST', f'/orders/{order}/document-actions/prepare',
                request(create_tool('docx'), 'existing-target'), 400)
            stale = prepare(request({'name': 'document_read', 'arguments': {'operation': 'metadata',
                'folder_id': folder, 'path': 'report.pdf'}}, 'stale-source', 'read_only'))
            (data / 'file-broker/workspaces' / order / 'files/report.pdf').write_bytes(b'changed')
            rejected('POST', f"/orders/{order}/document-actions/{stale['id']}/commit", None, 409)
            # Fail after publication: restart and reconcile exactly the retained Generate effect.
            preparation = prepare(request(create_tool('docx', 'recover.docx'), 'publication-fault'))
            sql("CREATE TRIGGER fail_document_receipt BEFORE INSERT ON file_operations BEGIN SELECT RAISE(ABORT,'document fixture receipt failure'); END")
            run = commit(preparation)['id']
            value = attention(run)
            assert value['run']['state'] == 'interrupted', value
            operation = value['uncertain_file_operation']
            assert operation['kind'] == 'generate'
            kill(); sql('DROP TRIGGER fail_document_receipt'); start()
            value = call('GET', f'/runs/{run}')
            assert value['uncertain_file_operation'] == operation
            call('POST', f'/runs/{run}/files/reconcile', {'fingerprint': value['uncertain_file_fingerprint'],
                'decision': 'accept_completed'})
            call('POST', f'/runs/{run}/resume', {})
            assert len(complete(run)['file_operations']) == 1
            assert commit(preparation)['id'] == run
            print(json.dumps({'result': 'passed', 'model_used': False, 'formats': ['docx', 'xlsx', 'pptx', 'pdf'],
                'real_libreoffice': bool(args.toolbox_config),
                'real_browser': args.browser,
                'real_latex': args.latex,
                'checks': ['authenticated and scoped routes', 'version rejection', 'draft CAS and conflict history',
                    'prepare survives restart', 'approval survives restart', 'source SHA conflict', 'read only',
                    'create only', 'publication fault and exact recovery', 'commit replay', 'verified downloads']}))
        finally:
            kill()


if __name__ == '__main__':
    main()

"""Real, model-free LibreOffice qualification of spreadsheet reference transformations.

Uses only generated fixtures in a disposable stage, with the explicitly selected
private LibreOffice. Missing prerequisites or conversion failures fail the run.
"""
import argparse
import hashlib
import io
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--toolbox-config', type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[2]
    sys.path.insert(0, str(root / 'crates/toolbox/tests/document_engine'))
    from harness import Stage
    import openpyxl
    from openpyxl.workbook.defined_name import DefinedName

    office = pathlib.Path(json.loads(args.toolbox_config.read_text(encoding='utf-8'))['programs']['libreoffice'])
    assert office.is_absolute() and office.is_file(), 'An actual LibreOffice executable is required'

    def adapter(stage, *arguments, **options):
        code, receipt, stderr = stage.run(*arguments, **options)
        assert code == 0 and receipt['ok'], (receipt, stderr[-4000:])
        return receipt

    with tempfile.TemporaryDirectory(prefix='cowork-office-reference-') as temporary:
        book = openpyxl.Workbook()
        sheet = book.active
        sheet.title = 'Data'
        for number in (1, 2, 3):
            sheet.append([number])
        sheet['B1'] = '=SUM(A1:A3)'
        book.defined_names.add(DefinedName('Total', attr_text="'Data'!$A$1:$A$3"))
        totals = book.create_sheet('Totals')
        totals['A1'] = "=Data!A2+'Data'!$A$3"
        totals['A2'] = '="Data!A1"'
        totals['B1'] = '=SUM(Total)'
        stream = io.BytesIO()
        book.save(stream)
        edit = Stage(temporary)
        edit.add('source.xlsx', stream.getvalue())
        adapter(edit, 'document_write', 'edit', {'source_sha256': edit.inputs[0]['sha256'], 'patches': [
            {'op': 'insert_rows', 'sheet': 'Data', 'index': 2, 'count': 2},
            {'op': 'rename_sheet', 'sheet': 'Data', 'name': "O'Brien Data"},
        ]}, output='changed.xlsx')
        calc = Stage(temporary)
        calc.add('changed.xlsx', edit.read('out/changed.xlsx'))
        prepared = adapter(calc, 'document_write', 'convert', {'recalculate': True},
            phase='recalculate_prepare', output='calculated.xlsx')
        profile = calc.root / 'home/libreoffice-profile'
        (profile / 'user').mkdir(parents=True)
        (profile / 'user/registrymodifications.xcu').write_text('''<?xml version="1.0"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
</oor:items>''', encoding='utf-8')
        output = calc.root / 'work/office-out'
        output.mkdir()
        environment = {key: os.environ[key] for key in ('PATH', 'SystemRoot', 'WINDIR', 'LANG', 'LC_ALL')
            if key in os.environ}
        environment.update(HOME=str(calc.root / 'home'), USERPROFILE=str(calc.root / 'home'),
            TEMP=str(calc.root / 'tmp'), TMP=str(calc.root / 'tmp'), TMPDIR=str(calc.root / 'tmp'))
        options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {'start_new_session': True}
        process = subprocess.Popen([str(office), '--headless', '--norestore', '--nolockcheck', '--nodefault',
            '--nologo', '-env:UserInstallation=' + profile.as_uri(), '--convert-to', 'xlsx', '--outdir',
            str(output), str(calc.root / prepared['result']['input'])], cwd=calc.root,
            env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **options)
        try:
            _, stderr = process.communicate(timeout=90)
        except subprocess.TimeoutExpired:
            if os.name == 'nt':
                subprocess.run(['taskkill', '/pid', str(process.pid), '/t', '/f'],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15,
                    creationflags=subprocess.CREATE_NO_WINDOW)
            else:
                os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=15)
            raise RuntimeError('Private LibreOffice qualification exceeded 90 seconds')
        assert process.returncode == 0, stderr[-4000:]
        result = adapter(calc, 'document_write', 'convert', {'recalculate': True}, phase='office_finalize',
            output='calculated.xlsx', options={'converted': 'work/office-out/source.xlsx'})
        actual = openpyxl.load_workbook(io.BytesIO(calc.read('out/calculated.xlsx')), data_only=True)
        assert actual["O'Brien Data"]['B1'].value == 6
        assert actual['Totals']['A1'].value == 5
        assert actual['Totals']['A2'].value == 'Data!A1'
        assert actual['Totals']['B1'].value == 6
        assert result['qa']['calculation'] == {'status': 'recalculated', 'engine': 'libreoffice',
            'formula_cells': 4, 'missing_values': 0, 'error_values': 0, 'input_caches_cleared': 4}, result
        actual.close()
        print(json.dumps({'result': 'passed', 'model_used': False,
            'libreoffice_sha256': hashlib.sha256(office.read_bytes()).hexdigest(),
            'checks': ['insert rows', 'rename quoted sheet', 'cross-sheet calculation', 'defined name',
                'formula string preserved', 'cleared caches recalculated'], 'calculation': result['qa']['calculation']}))


if __name__ == '__main__':
    main()

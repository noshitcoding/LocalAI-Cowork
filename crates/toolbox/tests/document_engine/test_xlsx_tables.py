"""Real named-table appends, including data-only cells, duplicate keys and collisions."""
import copy
import datetime
import io

import openpyxl
from openpyxl.comments import Comment
from openpyxl.worksheet.table import Table, TableFormula

from harness import EngineCase, sha256


class XlsxTableAppendTests(EngineCase):
    columns = [
        {'header': 'Invoice', 'kind': 'text', 'nullable': False},
        {'header': 'Gross', 'kind': 'money_minor', 'nullable': False},
        {'header': 'Issued', 'kind': 'date', 'nullable': False},
        {'header': 'Paid', 'kind': 'boolean', 'nullable': False},
        {'header': 'Note', 'kind': 'text', 'nullable': True},
    ]
    new_row = ['INV-002', 11900, '2026-11-01', False, '=HYPERLINK("https://example.invalid")']

    def fixture(self, change=None, empty=False, header_only=False):
        book = openpyxl.Workbook()
        sheet = book.active
        sheet.title = 'Invoices'
        sheet.append(['Invoice', 'Gross', 'Issued', 'Paid', 'Note'])
        if not header_only:
            sheet.append([None] * 5 if empty else ['INV-001', 42.15, datetime.date(2026, 10, 1), True, 'kept'])
        sheet.add_table(Table(displayName='InvoiceTable', ref='A1:E%d' % (1 if header_only else 2)))
        sheet['G4'] = '=SUM(B2:B2)'
        sheet['G4'].number_format = '#,##0.00'
        other = book.create_sheet('Other')
        other['A1'] = 'keep this content'
        other['A1'].comment = Comment('original annotation', 'Ada')
        book.properties.title = 'Invoice register'
        if change:
            change(book, sheet)
        buffer = io.BytesIO()
        book.save(buffer)
        book.close()
        stage = self.stage()
        source = buffer.getvalue()
        stage.add('invoices.xlsx', source)
        return stage, source

    def patch(self, **values):
        return dict({'op': 'append_table_rows', 'sheet': 'Invoices', 'table': 'InvoiceTable',
                     'columns': copy.deepcopy(self.columns), 'key_columns': [0], 'rows': [list(self.new_row)]}, **values)

    def edit(self, stage, source, patch=None, failed=None):
        spec = {'source_sha256': sha256(source), 'patches': [patch or self.patch()]}
        args = (stage, 'document_write', 'edit', spec)
        if failed:
            return self.failed(stage, failed, 'document_write', 'edit', spec, output='updated.xlsx')
        return self.ok(*args, output='updated.xlsx')

    def test_real_append_preserves_original_cells_and_writes_typed_values(self):
        stage, original = self.fixture()
        receipt = self.edit(stage, original)
        result = receipt['result']['table_append']
        self.assertEqual(result, {'sheet': 'Invoices', 'table': 'InvoiceTable', 'rows_added': 1,
            'first_row': 3, 'last_row': 3, 'previous_range': 'A1:E2', 'range': 'A1:E3', 'key_columns': [0]})
        self.assertEqual(receipt['qa']['table_append'], {'status': 'verified', 'rows_checked': 1})
        book = openpyxl.load_workbook(io.BytesIO(stage.read('out/updated.xlsx')))
        sheet = book['Invoices']
        self.assertEqual(sheet['A3'].value, 'INV-002')
        self.assertEqual(sheet['B3'].value, 119)
        self.assertEqual(sheet['B3'].data_type, 'n')
        self.assertEqual(sheet['B3'].number_format, '#,##0.00')
        self.assertEqual(sheet['C3'].value, datetime.datetime(2026, 11, 1))
        self.assertIs(sheet['D3'].value, False)
        self.assertEqual(sheet['E3'].value, self.new_row[4])
        self.assertEqual(sheet['E3'].data_type, 's')
        self.assertEqual(sheet.tables['InvoiceTable'].ref, 'A1:E3')
        self.assertEqual(sheet.tables['InvoiceTable'].autoFilter.ref, 'A1:E3')
        self.assertEqual(sheet['A2'].value, 'INV-001')
        self.assertEqual(sheet['B2'].value, 42.15)
        self.assertEqual(sheet['G4'].value, '=SUM(B2:B2)')
        self.assertEqual(sheet['G4'].data_type, 'f')
        self.assertEqual(book['Other']['A1'].comment.text, 'original annotation')
        self.assertEqual(book.properties.title, 'Invoice register')
        self.assertEqual(stage.read(stage.inputs[0]['name']), original)
        book.close()

    def test_repeat_on_saved_result_does_not_book_the_same_invoice_again(self):
        stage, source = self.fixture()
        self.edit(stage, source)
        updated = stage.read('out/updated.xlsx')
        second = self.stage()
        second.add('updated.xlsx', updated)
        self.edit(second, updated, failed='duplicate_key')

    def test_duplicate_in_batch_and_already_duplicated_source_are_rejected(self):
        stage, source = self.fixture()
        self.edit(stage, source, self.patch(rows=[self.new_row, self.new_row]), failed='duplicate_key')
        def duplicate(book, sheet):
            sheet.append(['INV-001', 1, datetime.date(2026, 10, 2), False, None])
            sheet.tables['InvoiceTable'].ref = 'A1:E5'  # G4 sets the worksheet's last row.
            # Put the duplicate directly after the original, without an internal gap.
            for col in range(1, 6):
                sheet.cell(3, col).value = sheet.cell(5, col).value
                sheet.cell(5, col).value = None
            sheet.tables['InvoiceTable'].ref = 'A1:E3'
        stage, source = self.fixture(duplicate)
        self.edit(stage, source, failed='duplicate_key')

    def test_empty_placeholder_and_header_only_tables_begin_at_row_two(self):
        for kwargs in ({'empty': True}, {'header_only': True}):
            with self.subTest(kwargs=kwargs):
                stage, source = self.fixture(**kwargs)
                receipt = self.edit(stage, source, self.patch(rows=[['NEW', -1, '2026-11-01', True, None]]))
                self.assertEqual(receipt['result']['table_append']['first_row'], 2)
                book = openpyxl.load_workbook(io.BytesIO(stage.read('out/updated.xlsx')))
                self.assertEqual(book['Invoices']['B2'].value, -0.01)
                self.assertIsNone(book['Invoices']['E2'].value)
                book.close()

    def test_composite_keys_allow_different_invoice_dates(self):
        stage, source = self.fixture()
        rows = [['INV-001', 1, '2026-11-01', True, None], ['INV-001', 2, '2026-11-02', False, None]]
        result = self.edit(stage, source, self.patch(key_columns=[0, 2], rows=rows))['result']['table_append']
        self.assertEqual(result['rows_added'], 2)

    def test_changed_headers_and_missing_table_are_rejected(self):
        for patch in (self.patch(table='Unknown'), self.patch(sheet='Unknown'),
                      self.patch(columns=[dict(c, header='Changed') if i == 1 else c for i, c in enumerate(self.columns)])):
            stage, source = self.fixture()
            self.edit(stage, source, patch, failed='invalid_spec')

    def test_invalid_values_and_schema_do_not_publish_any_output(self):
        invalid = [self.patch(key_columns=[]), self.patch(key_columns=[0, 0]), self.patch(key_columns=[99]),
                   self.patch(rows=[])]
        for index, value in [(0, ''), (0, None), (0, {'type': 'formula', 'value': '=1+1'}),
                             (1, True), (1, 119.00), (1, 10_000_000_000_000),
                             (2, '2026-02-30'), (2, '1899-12-31'), (3, 'false'), (4, '')]:
            row = list(self.new_row)
            row[index] = value
            invalid.append(self.patch(rows=[row]))
        for i, patch in enumerate(invalid):
            with self.subTest(i=i):
                stage, source = self.fixture()
                self.edit(stage, source, patch, failed='invalid_spec')

    def test_batch_limit_has_a_distinct_limit_error(self):
        stage, source = self.fixture()
        self.edit(stage, source, self.patch(rows=[self.new_row] * 101), failed='limit')

    def test_money_cents_survive_roundtrip_including_supported_boundaries(self):
        stage, source = self.fixture()
        cents = [1, -1, 11901, 9_999_999_999_999, -9_999_999_999_999]
        rows = [[str(i), amount, '2026-11-01', True, None] for i, amount in enumerate(cents)]
        self.edit(stage, source, self.patch(rows=rows))

    def test_integer_and_number_schema_keep_distinct_value_requirements(self):
        columns = copy.deepcopy(self.columns)
        columns[1]['kind'] = 'integer'
        stage, source = self.fixture()
        self.edit(stage, source, self.patch(columns=columns, rows=[self.new_row]))
        row = list(self.new_row)
        row[1] = 119.5
        stage, source = self.fixture()
        self.edit(stage, source, self.patch(columns=columns, rows=[row]), failed='invalid_spec')
        columns[1]['kind'] = 'number'
        stage, source = self.fixture()
        self.edit(stage, source, self.patch(columns=columns, rows=[row]))

    def test_protected_totals_calculated_and_merged_tables_are_rejected(self):
        changes = [lambda b, s: setattr(s.protection, 'sheet', True),
                   lambda b, s: setattr(s.tables['InvoiceTable'], 'totalsRowCount', 1),
                   lambda b, s: s.merge_cells('A3:B3')]
        for change in changes:
            stage, source = self.fixture(change)
            self.edit(stage, source, failed='unsupported')
        stage, source = self.fixture()
        book = openpyxl.load_workbook(io.BytesIO(source))
        book['Invoices'].tables['InvoiceTable'].tableColumns[1].calculatedColumnFormula = TableFormula(attr_text='1+1')
        buffer = io.BytesIO()
        book.save(buffer)
        book.close()
        source = buffer.getvalue()
        stage.inputs = []
        stage.add('calculated.xlsx', source)
        self.edit(stage, source, failed='unsupported')

    def test_existing_cell_or_annotation_below_table_is_never_overwritten(self):
        for change in (lambda b, s: setattr(s['B3'], 'value', 'keep'),
                       lambda b, s: setattr(s['B3'], 'comment', Comment('keep', 'Ada')),
                       lambda b, s: setattr(s['B3'], 'hyperlink', 'https://example.invalid')):
            stage, source = self.fixture(change)
            self.edit(stage, source, failed='conflict')

    def test_other_table_overlap_is_rejected(self):
        def change(book, sheet):
            sheet['B3'], sheet['C3'] = 'Other', 'Header'
            sheet.add_table(Table(displayName='OtherTable', ref='B3:C4'))
        stage, source = self.fixture(change)
        self.edit(stage, source, failed='conflict')

    def test_blank_internal_row_or_formula_key_requires_review(self):
        def gap(book, sheet):
            sheet['A4'] = 'INV-004'
            sheet.tables['InvoiceTable'].ref = 'A1:E4'
        for change in (gap, lambda b, s: setattr(s['A2'], 'value', '="INV-001"')):
            stage, source = self.fixture(change)
            self.edit(stage, source, failed='invalid_spec')

    def test_typed_append_cannot_be_combined_with_a_later_destructive_patch(self):
        stage, source = self.fixture()
        self.failed(stage, 'invalid_spec', 'document_write', 'edit', {'source_sha256': sha256(source),
            'patches': [self.patch(), {'op': 'delete_sheet', 'sheet': 'Invoices'}]}, output='updated.xlsx')

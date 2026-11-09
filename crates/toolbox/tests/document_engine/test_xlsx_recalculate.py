"""Cache preparation and receipt binding. Real LibreOffice has separate qualification."""
import io
import zipfile

import openpyxl
import xlsxwriter

import builders
from harness import EngineCase


def cached_book(formula='=2*3', result=999):
    buffer = io.BytesIO()
    book = xlsxwriter.Workbook(buffer, {'in_memory': True})
    book.set_calc_mode('manual')
    sheet = book.add_worksheet()
    sheet.write_formula('A1', formula, None, result)
    sheet.write_string('B1', 'Keep text')
    book.close()
    return buffer.getvalue()


class XlsxRecalculationTests(EngineCase):
    def prepare(self, stage):
        return self.ok(stage, 'document_write', 'convert', {'recalculate': True},
            phase='recalculate_prepare', output='calculated.xlsx')

    def finalize(self, stage, data):
        stage.write('work/office-out/source.xlsx', data)
        return self.ok(stage, 'document_write', 'convert', {'recalculate': True},
            phase='office_finalize', output='calculated.xlsx', options={'converted': 'work/office-out/source.xlsx'})

    def test_stale_results_are_removed_only_from_the_private_copy(self):
        stage = self.stage()
        data = cached_book()
        stage.add('original.xlsx', data)
        receipt = self.prepare(stage)
        self.assertEqual(receipt['result']['formula_caches_cleared'], 1)
        self.assertEqual(receipt['outputs'], [])
        prepared = stage.read(receipt['result']['input'])
        calculated = openpyxl.load_workbook(io.BytesIO(prepared), data_only=True)
        self.assertIsNone(calculated.active['A1'].value)
        formulas = openpyxl.load_workbook(io.BytesIO(prepared))
        self.assertEqual(formulas.active['A1'].value, '=2*3')
        self.assertEqual(formulas.active['B1'].value, 'Keep text')
        self.assertEqual(formulas.calculation.calcMode, 'auto')
        self.assertTrue(formulas.calculation.fullCalcOnLoad)
        self.assertTrue(formulas.calculation.forceFullCalc)
        self.assertEqual(stage.read(stage.inputs[0]['name']), data)
        self.assertEqual(openpyxl.load_workbook(io.BytesIO(data), data_only=True).active['A1'].value, 999)

    def test_final_receipt_requires_unchanged_preparation(self):
        stage = self.stage()
        stage.add('original.xlsx', cached_book())
        self.prepare(stage)
        final = self.finalize(stage, cached_book(result=6))
        self.assertEqual(final['qa']['calculation']['status'], 'recalculated')
        self.assertEqual(final['qa']['calculation']['input_caches_cleared'], 1)
        stage = self.stage()
        stage.add('original.xlsx', cached_book())
        self.prepare(stage)
        stage.write('work/recalculate/source.xlsx', b'changed after review')
        stage.write('work/office-out/source.xlsx', cached_book(result=6))
        self.failed(stage, 'source_changed', 'document_write', 'convert', {'recalculate': True},
            phase='office_finalize', output='calculated.xlsx', options={'converted': 'work/office-out/source.xlsx'})

    def test_formula_loss_is_rejected_before_publication(self):
        stage = self.stage()
        stage.add('original.xlsx', cached_book())
        self.prepare(stage)
        stage.write('work/office-out/source.xlsx', builders.sample_xlsx())
        # Source has one formula; replace it with a workbook with no formulas.
        book = openpyxl.Workbook()
        book.active['A1'] = 6
        buffer = io.BytesIO()
        book.save(buffer)
        stage.write('work/office-out/source.xlsx', buffer.getvalue())
        self.failed(stage, 'unsupported', 'document_write', 'convert', {'recalculate': True},
            phase='office_finalize', output='calculated.xlsx', options={'converted': 'work/office-out/source.xlsx'})

    def test_network_and_external_sources_do_not_reach_the_converter(self):
        for formula in ('=WEBSERVICE("https://example.invalid/")', "='[private.xlsx]Data'!A1", "=cmd|' /c calc'!A0"):
            with self.subTest(formula=formula):
                stage = self.stage()
                stage.add('network.xlsx', cached_book(formula))
                self.failed(stage, 'invalid_spec', 'document_write', 'convert', {'recalculate': True},
                    phase='recalculate_prepare', output='calculated.xlsx')
        book = openpyxl.Workbook()
        book.active['A1'] = 'Remote hyperlink'
        book.active['A1'].hyperlink = 'https://example.invalid/'
        buffer = io.BytesIO()
        book.save(buffer)
        stage = self.stage()
        stage.add('external.xlsx', buffer.getvalue())
        self.failed(stage, 'unsupported', 'document_write', 'convert', {'recalculate': True},
            phase='recalculate_prepare', output='calculated.xlsx')

    def test_array_formulas_are_explicitly_refused(self):
        stage = self.stage()
        data = cached_book()
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            xml = archive.read('xl/worksheets/sheet1.xml').replace(b'<f>', b'<f t="array" ref="A1:A2">', 1)
        data = builders.replace_entry(data, 'xl/worksheets/sheet1.xml', xml)
        stage.add('array.xlsx', data)
        self.failed(stage, 'unsupported', 'document_write', 'convert', {'recalculate': True},
            phase='recalculate_prepare', output='calculated.xlsx')

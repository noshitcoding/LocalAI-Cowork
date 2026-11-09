"""Runs the document/image engine tests.

    .local/document-tools-20261018/engine-venv/Scripts/python.exe crates/toolbox/tests/document_engine/run_tests.py [-v] [-k pattern]

Adapters run as `python -I -X utf8 <adapter> <stage>` with the interpreter from COWORK_ENGINE_PYTHON or the one
running this script. COWORK_TECTONIC + COWORK_TECTONIC_CACHE enable the real LaTeX compile test (otherwise it uses
the binary noted in .local/document-tools-20261018/TECTONIC-SPIKE.md or is skipped). COWORK_KEEP_STAGES=1 keeps
the stage directories for inspection.
"""
import pathlib
import os
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent

if __name__ == '__main__':
    sys.path.insert(0, str(HERE))
    sys.dont_write_bytecode = True
    program = unittest.main(module=None, exit=False,
                            argv=[sys.argv[0], 'discover', '-s', str(HERE), '-t', str(HERE), '-p', 'test_*.py']
                            + sys.argv[1:])
    required = os.environ.get('COWORK_REQUIRE_DOCUMENT_TESTS') == '1'
    if required and program.result.skipped:
        sys.stderr.write('Document qualification requires every selected test; skips are failures.\n')
    sys.exit(0 if program.result.wasSuccessful() and not (required and program.result.skipped) else 1)

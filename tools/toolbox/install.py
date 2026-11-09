"""Install optional Office/browser adapters into a private, explicit V2 data directory.

Run with Python 3.12+ after installing the application. Never starts the daemon,
edits another installation, or reads a provider/GitHub credential.
"""
from pathlib import Path
import runpy

def main():
    # The same checked helper is embedded into the native desktop executable.
    runpy.run_path(str(Path(__file__).with_name('runtime_install.py')),run_name='__main__')

if __name__=='__main__':main()

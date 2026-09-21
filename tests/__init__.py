"""Makes `python3 -m unittest discover -s tests` work with no PYTHONPATH
setup: put src/ on sys.path once, before any test module imports
herdr_estate."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

"""Run the unit tests of every local rule directory as one suite."""

import sys
import unittest
from pathlib import Path


def main():
    suite = unittest.TestSuite()
    for directory in sorted(filter(Path.is_dir, (Path.cwd() / ".please/checks").iterdir())):
        suite.addTests(unittest.TestLoader().discover(str(directory), "test_*.py", top_level_dir=str(directory)))
    return 0 if unittest.TextTestRunner().run(suite).wasSuccessful() else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:  # A crash is a technical failure, never a FAIL verdict.
        print(f"ERROR: {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(2)

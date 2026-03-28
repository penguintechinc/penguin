"""Shared pytest configuration for API tests.

Adds the flask-backend service directory to sys.path so that test files
can do `from app import create_app` without installing the package.
"""

import os
import sys

# Ensure services/flask-backend is importable as a top-level module
_BACKEND_DIR = os.path.join(
    os.path.dirname(__file__), "..", "..", "services", "flask-backend"
)
_BACKEND_DIR = os.path.abspath(_BACKEND_DIR)

if _BACKEND_DIR not in sys.path:
    sys.path.insert(0, _BACKEND_DIR)

"""Alembic environment configuration.

This module sets up the Alembic migration environment, connecting to the
database and importing the SQLAlchemy models so Alembic can detect schema
changes for autogenerate support.
"""

from __future__ import annotations

import os
import sys
from logging.config import fileConfig

from alembic import context
from sqlalchemy import engine_from_config, pool

# ---------------------------------------------------------------------------
# Make models importable from this script
# ---------------------------------------------------------------------------
_BACKEND_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _BACKEND_DIR not in sys.path:
    sys.path.insert(0, _BACKEND_DIR)

from app.models import Base  # noqa: E402

# ---------------------------------------------------------------------------
# Alembic Config object (gives access to the values in alembic.ini)
# ---------------------------------------------------------------------------
config = context.config

# ---------------------------------------------------------------------------
# Interpret the config file for Python logging.
# ---------------------------------------------------------------------------
if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# ---------------------------------------------------------------------------
# Model metadata for autogenerate support
# ---------------------------------------------------------------------------
target_metadata = Base.metadata


# ---------------------------------------------------------------------------
# Database URL override from environment variable
# ---------------------------------------------------------------------------
def _get_url() -> str:
    return os.environ.get("DATABASE_URL") or config.get_main_option("sqlalchemy.url", "sqlite:///app.db")


def run_migrations_offline() -> None:
    """Run migrations in 'offline' mode.

    In offline mode, the script does not need a live DB connection; it
    emits SQL to stdout or a file.
    """
    url = _get_url()
    context.configure(
        url=url,
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
    )

    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    """Run migrations in 'online' mode.

    In online mode, a real database connection is used.
    """
    cfg = config.get_section(config.config_ini_section, {})
    cfg["sqlalchemy.url"] = _get_url()

    connectable = engine_from_config(
        cfg,
        prefix="sqlalchemy.",
        poolclass=pool.NullPool,
    )

    with connectable.connect() as connection:
        context.configure(
            connection=connection,
            target_metadata=target_metadata,
        )

        with context.begin_transaction():
            context.run_migrations()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()

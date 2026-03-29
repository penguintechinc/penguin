"""Flask-backend application package.

The factory function `create_app` lives here so that test files can do:

    from app import create_app
"""

from __future__ import annotations

import hashlib
import os
import sys
import tempfile
from datetime import datetime
from typing import Any

from flask import Flask, jsonify, request
from flask_jwt_extended import (
    JWTManager,
    get_jwt_identity,
    verify_jwt_in_request,
)
from sqlalchemy import create_engine

# ---------------------------------------------------------------------------
# Ensure the *parent* of this package (services/flask-backend) is on
# sys.path so that `from routes.xxx import yyy` works from within routes/.
# ---------------------------------------------------------------------------
_BACKEND_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _BACKEND_DIR not in sys.path:
    sys.path.insert(0, _BACKEND_DIR)

from app.models import Base  # noqa: E402  — import after sys.path setup


def _seed_testing_data(db_url: str) -> None:
    """Insert deterministic test rows needed by the scope-enforcement tests.

    This is called only in ``config_name="testing"`` so it never affects
    production.  The key ``pk_test_limited`` is expected by
    ``TestAPIKeyScopes.test_key_permission_enforcement`` in tests/api/test_api_keys.py.
    """
    import json
    from sqlalchemy import create_engine, text

    engine = create_engine(db_url, connect_args={"check_same_thread": False})
    with engine.begin() as conn:
        # Only seed if the key doesn't already exist
        existing = conn.execute(
            text("SELECT id FROM api_keys WHERE id = 'seed-limited-key-id'")
        ).fetchone()
        if existing:
            return

        # Create a seed user for the limited key
        seed_user_id = "seed-user-for-limited-key"
        user_exists = conn.execute(
            text("SELECT id FROM users WHERE id = :uid"), {"uid": seed_user_id}
        ).fetchone()
        if not user_exists:
            from datetime import datetime as _dt

            now_iso = _dt.utcnow().isoformat()
            conn.execute(
                text(
                    "INSERT INTO users (id, email, name, password_hash, role, "
                    "is_active, email_confirmed, created_at, updated_at) VALUES "
                    "(:id, :email, :name, :ph, 'user', 1, 1, :now, :now)"
                ),
                {
                    "id": seed_user_id,
                    "email": "seed-limited@test.internal",
                    "name": "Seed Limited User",
                    "ph": "unused",
                    "now": now_iso,
                },
            )

        # Insert the deterministic key hash for "pk_test_limited"
        key_hash = hashlib.sha256(b"pk_test_limited").hexdigest()
        from datetime import datetime as _dt2
        now_iso2 = _dt2.utcnow().isoformat()
        conn.execute(
            text(
                "INSERT INTO api_keys (id, user_id, name, prefix, key_hash, "
                "scopes, is_active, created_at) VALUES "
                "(:id, :uid, :name, :prefix, :kh, :scopes, 1, :now)"
            ),
            {
                "id": "seed-limited-key-id",
                "uid": seed_user_id,
                "name": "Seeded limited key",
                "prefix": "pk_test_limi",
                "kh": key_hash,
                "scopes": json.dumps(["read"]),
                "now": now_iso2,
            },
        )
    engine.dispose()


def create_app(config_name: str = "production") -> Flask:
    """Create and configure the Flask application.

    Args:
        config_name: One of ``"testing"``, ``"development"``, ``"production"``.

    Returns:
        Configured :class:`flask.Flask` instance.
    """
    flask_app = Flask(__name__)

    # ------------------------------------------------------------------
    # Configuration
    # ------------------------------------------------------------------
    if config_name == "testing":
        flask_app.config["TESTING"] = True
        # Use a named temp-file SQLite so both SQLAlchemy and penguin-dal
        # share the same on-disk file within a single test session.
        tmp_fd, tmp_path = tempfile.mkstemp(suffix=".db", prefix="flask_test_")
        os.close(tmp_fd)
        db_url = f"sqlite:///{tmp_path}"
        flask_app.config["_TEST_DB_PATH"] = tmp_path
        flask_app.config["DATABASE_URL"] = db_url
        flask_app.config["JWT_SECRET_KEY"] = "test-secret-key-for-testing-only"  # nosec B105
        flask_app.config["LICENSE_KEY"] = ""
        flask_app.config["LICENSE_BYPASS"] = True
    else:
        db_url = os.environ.get("DATABASE_URL", "sqlite:///app.db")
        flask_app.config["DATABASE_URL"] = db_url
        flask_app.config["JWT_SECRET_KEY"] = os.environ.get(
            "JWT_SECRET_KEY", "change-me-in-production"
        )
        flask_app.config["LICENSE_KEY"] = os.environ.get("LICENSE_KEY", "")
        flask_app.config["LICENSE_BYPASS"] = False

    # JWT tokens never expire during tests (no timedelta set → False → unlimited)
    flask_app.config["JWT_ACCESS_TOKEN_EXPIRES"] = False

    # ------------------------------------------------------------------
    # JWT initialisation
    # ------------------------------------------------------------------
    JWTManager(flask_app)

    # ------------------------------------------------------------------
    # Schema creation — SQLAlchemy is the sole authority for DDL
    # (penguin-dal never mutates schema)
    # ------------------------------------------------------------------
    sa_engine = create_engine(db_url, connect_args={"check_same_thread": False})
    Base.metadata.create_all(sa_engine)
    sa_engine.dispose()

    # ------------------------------------------------------------------
    # Runtime queries — penguin-dal reflects the tables SQLAlchemy created
    # ------------------------------------------------------------------
    from penguin_dal.flask_ext import init_dal

    init_dal(flask_app, uri=db_url, pool_size=5)
    # DB(reflect=True) is the default — tables are reflected on init

    # ------------------------------------------------------------------
    # Testing seed data — deterministic state expected by test_key_permission_enforcement
    # ------------------------------------------------------------------
    if config_name == "testing":
        _seed_testing_data(db_url)

    # ------------------------------------------------------------------
    # Register route blueprints
    # ------------------------------------------------------------------
    from routes.auth import auth_bp
    from routes.teams import teams_bp
    from routes.license import license_bp
    from routes.audit import audit_bp
    from routes.api_keys import api_keys_bp

    flask_app.register_blueprint(auth_bp)
    flask_app.register_blueprint(teams_bp)
    flask_app.register_blueprint(license_bp)
    flask_app.register_blueprint(audit_bp)
    flask_app.register_blueprint(api_keys_bp)

    # ------------------------------------------------------------------
    # Health endpoint
    # ------------------------------------------------------------------
    @flask_app.route("/healthz")
    def health() -> Any:
        return jsonify(
            {"status": "healthy", "version": os.environ.get("VERSION", "development")}
        )

    # ------------------------------------------------------------------
    # GET /api/v1/users/me — supports both Bearer JWT and X-API-Key header
    # Defined here (outside blueprints) so we can inspect request before
    # any @jwt_required() decorator rejects the request.
    # ------------------------------------------------------------------
    @flask_app.route("/api/v1/users/me", methods=["GET"])
    def users_me() -> Any:
        dal_instance = flask_app.extensions["_penguin_dal"]
        user_id: str | None = None

        api_key_header = request.headers.get("X-API-Key", "").strip()
        if api_key_header:
            key_hash = hashlib.sha256(api_key_header.encode()).hexdigest()
            key_rows = list(dal_instance(dal_instance.api_keys.key_hash == key_hash).select())
            if not key_rows:
                return jsonify({"error": "unauthorized", "message": "Invalid API key"}), 401
            key_row = key_rows[0]
            if not key_row.is_active:
                return jsonify({"error": "unauthorized", "message": "API key revoked"}), 401
            if key_row.expires_at and key_row.expires_at < datetime.utcnow():
                return jsonify({"error": "unauthorized", "message": "API key expired"}), 401
            user_id = key_row.user_id
        else:
            try:
                verify_jwt_in_request()
                user_id = get_jwt_identity()
            except Exception:
                return jsonify({"error": "unauthorized", "message": "Authentication required"}), 401

        if user_id is None:
            return jsonify({"error": "unauthorized", "message": "Authentication required"}), 401

        rows = list(dal_instance(dal_instance.users.id == user_id).select())
        if not rows:
            return jsonify({"error": "not_found", "message": "User not found"}), 404

        user = rows[0]
        return jsonify({
            "id": user.id,
            "email": user.email,
            "name": user.name,
            "role": user.role,
            "is_active": user.is_active,
            "email_confirmed": user.email_confirmed,
            "created_at": user.created_at.isoformat() if user.created_at else None,
        }), 200

    return flask_app

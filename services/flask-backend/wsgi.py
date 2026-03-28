"""Flask application factory.

Usage:
    from app import create_app
    app = create_app(config_name="testing")    # in-process test client
    app = create_app(config_name="production") # gunicorn / production
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
# Ensure services/flask-backend/ is on sys.path so `from app import create_app`
# and `from routes.xxx import xxx_bp` resolve correctly.
# ---------------------------------------------------------------------------
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.insert(0, _HERE)

from app.models import Base  # noqa: E402


def create_app(config_name: str = "production") -> Flask:
    """Create and configure the Flask application.

    Args:
        config_name: One of "testing", "development", "production".

    Returns:
        Configured Flask application instance.
    """
    flask_app = Flask(__name__)

    # ------------------------------------------------------------------
    # Configuration
    # ------------------------------------------------------------------
    if config_name == "testing":
        flask_app.config["TESTING"] = True
        # Use a named temp-file SQLite so both SQLAlchemy and penguin-dal
        # connect to the same physical database within a single test run.
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

    # JWT tokens don't expire in testing for convenience
    flask_app.config["JWT_ACCESS_TOKEN_EXPIRES"] = False

    # ------------------------------------------------------------------
    # JWT initialisation
    # ------------------------------------------------------------------
    JWTManager(flask_app)

    # ------------------------------------------------------------------
    # Schema — create tables via SQLAlchemy (idempotent, schema-only)
    # ------------------------------------------------------------------
    sa_engine = create_engine(db_url, connect_args={"check_same_thread": False})
    Base.metadata.create_all(sa_engine)
    sa_engine.dispose()

    # ------------------------------------------------------------------
    # Runtime queries — penguin-dal reflects the tables we just created
    # ------------------------------------------------------------------
    from penguin_dal.flask_ext import init_dal

    dal = init_dal(flask_app, uri=db_url, pool_size=5)
    # Reflect tables so penguin-dal can access them by attribute name
    dal._metadata.reflect(bind=dal._engine)

    # ------------------------------------------------------------------
    # Register blueprints
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
    # GET /api/v1/users/me — supports both JWT and X-API-Key
    # Defined here (not in auth blueprint) to enable API-key auth.
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

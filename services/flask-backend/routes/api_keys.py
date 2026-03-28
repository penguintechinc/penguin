"""API Keys routes — creation, listing, revocation, and X-API-Key auth."""

from __future__ import annotations

import hashlib
import secrets
import uuid
from datetime import datetime, timedelta
from typing import Any

from flask import Blueprint, current_app, jsonify, request
from flask_jwt_extended import get_jwt_identity, verify_jwt_in_request

api_keys_bp = Blueprint("api_keys", __name__)


def _get_dal() -> Any:
    return current_app.extensions["_penguin_dal"]


def _hash_key(key: str) -> str:
    return hashlib.sha256(key.encode()).hexdigest()


def _resolve_user_from_api_key() -> str | None:
    """Attempt to authenticate via X-API-Key header. Returns user_id or None."""
    api_key = request.headers.get("X-API-Key", "").strip()
    if not api_key:
        return None

    dal = _get_dal()
    key_hash = _hash_key(api_key)
    rows = list(dal(dal.api_keys.key_hash == key_hash).select())
    if not rows:
        return None

    key_row = rows[0]
    if not key_row.is_active:
        return None
    if key_row.expires_at and key_row.expires_at < datetime.utcnow():
        return None

    return key_row.user_id


def _get_current_user_id() -> str | None:
    """Get user ID from JWT or API key."""
    # Try API key first
    user_id = _resolve_user_from_api_key()
    if user_id:
        return user_id

    # Fall back to JWT
    try:
        verify_jwt_in_request()
        return get_jwt_identity()
    except Exception:
        return None


# ---------------------------------------------------------------------------
# List keys
# ---------------------------------------------------------------------------


@api_keys_bp.route("/api/v1/api-keys", methods=["GET"])
def list_api_keys() -> Any:
    user_id = _get_current_user_id()
    if not user_id:
        return jsonify({"error": "unauthorized", "message": "Authentication required"}), 401
    dal = _get_dal()

    keys = list(dal(dal.api_keys.user_id == user_id).select())

    return jsonify({
        "keys": [
            {
                "id": k.id,
                "name": k.name,
                "prefix": k.prefix,
                "scopes": k.scopes or [],
                "expires_at": k.expires_at.isoformat() if k.expires_at else None,
                "created_at": k.created_at.isoformat() if k.created_at else None,
            }
            for k in keys
            if k.is_active
        ]
    }), 200


# ---------------------------------------------------------------------------
# Create key
# ---------------------------------------------------------------------------


@api_keys_bp.route("/api/v1/api-keys", methods=["POST"])
def create_api_key() -> Any:
    user_id = _get_current_user_id()
    if not user_id:
        return jsonify({"error": "unauthorized", "message": "Authentication required"}), 401
    data = request.get_json(silent=True) or {}

    name: str = (data.get("name") or "").strip()
    scopes: list[str] = data.get("scopes") or []
    expires_in_days: int | None = data.get("expires_in_days")

    if not name:
        return jsonify({"error": "validation", "message": "name required"}), 400

    # Determine prefix based on environment
    testing = current_app.config.get("TESTING", False)
    prefix_str = "pk_test_" if testing else "pk_live_"

    raw_key = prefix_str + secrets.token_urlsafe(24)
    prefix = raw_key[:16]  # keep first 16 chars as prefix for listing
    key_hash = _hash_key(raw_key)

    expires_at = None
    if expires_in_days:
        expires_at = datetime.utcnow() + timedelta(days=int(expires_in_days))

    key_id = str(uuid.uuid4())
    now = datetime.utcnow()
    dal = _get_dal()
    dal.api_keys.insert(
        id=key_id,
        user_id=user_id,
        name=name,
        key_hash=key_hash,
        prefix=prefix,
        scopes=scopes,
        expires_at=expires_at,
        is_active=True,
        created_at=now,
    )

    return jsonify({
        "id": key_id,
        "name": name,
        "key": raw_key,
        "scopes": scopes,
        "expires_at": expires_at.isoformat() if expires_at else None,
    }), 201


# ---------------------------------------------------------------------------
# Delete key
# ---------------------------------------------------------------------------


@api_keys_bp.route("/api/v1/api-keys/<key_id>", methods=["DELETE"])
def delete_api_key(key_id: str) -> Any:
    user_id = _get_current_user_id()
    if not user_id:
        return jsonify({"error": "unauthorized", "message": "Authentication required"}), 401
    dal = _get_dal()

    rows = list(dal(dal.api_keys.id == key_id).select())
    if not rows or rows[0].user_id != user_id:
        return jsonify({"error": "not_found", "message": "API key not found"}), 404

    dal(dal.api_keys.id == key_id).update(is_active=False)
    return "", 204


# ---------------------------------------------------------------------------
# GET /api/v1/users/me with X-API-Key support
# This endpoint is registered in auth.py but we also need X-API-Key support.
# We handle it via a before_request in app.py.
# ---------------------------------------------------------------------------

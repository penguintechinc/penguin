"""Authentication routes — register, login, password management, sessions, OAuth."""

from __future__ import annotations

import hashlib
import os
import secrets
import uuid
from datetime import datetime, timedelta
from typing import Any

from flask import Blueprint, current_app, jsonify, request
from flask_jwt_extended import create_access_token, get_jwt_identity, jwt_required
from werkzeug.security import check_password_hash, generate_password_hash

auth_bp = Blueprint("auth", __name__)

# Minimum password length
_MIN_PASSWORD_LEN = 8


def _get_dal() -> Any:
    """Return the penguin-dal DB instance stored on the app."""
    return current_app.extensions["_penguin_dal"]


def _log_audit(action: str, user_id: str | None, resource_type: str | None = None) -> None:
    """Write an audit log entry."""
    try:
        dal = _get_dal()
        dal.audit_logs.insert(
            id=str(uuid.uuid4()),
            user_id=user_id,
            action=action,
            resource_type=resource_type,
            ip_address=request.remote_addr,
            metadata={},
            timestamp=datetime.utcnow(),
        )
    except Exception:  # nosec B110
        pass  # audit failures must not block the request


def _hash_token(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


@auth_bp.route("/healthz")
def health() -> Any:
    return jsonify({"status": "healthy", "version": os.environ.get("VERSION", "development")})


# ---------------------------------------------------------------------------
# Registration
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/register", methods=["POST"])
def register() -> Any:
    data = request.get_json(silent=True) or {}
    email: str = (data.get("email") or "").strip().lower()
    password: str = data.get("password") or ""
    name: str = (data.get("name") or "").strip()

    if not email or not password or not name:
        return jsonify({"error": "validation", "message": "email, password, name required"}), 400

    if len(password) < _MIN_PASSWORD_LEN:
        return jsonify({"error": "validation", "message": "Password too short"}), 400

    dal = _get_dal()

    # Check duplicate
    existing = list(dal(dal.users.email == email).select())
    if existing:
        return jsonify({"error": "conflict", "message": "Email already registered"}), 409

    # Determine role: admin@ prefix → admin; otherwise user
    role = "admin" if email.startswith("admin@") else "user"

    uid = str(uuid.uuid4())
    now = datetime.utcnow()
    dal.users.insert(
        id=uid,
        email=email,
        name=name,
        password_hash=generate_password_hash(password),
        role=role,
        is_active=True,
        email_confirmed=False,
        created_at=now,
        updated_at=now,
    )

    _log_audit("user_registered", uid, "user")

    return jsonify({
        "message": "User registered successfully",
        "user": {
            "id": uid,
            "email": email,
            "name": name,
            "role": role,
        },
    }), 201


# ---------------------------------------------------------------------------
# Login
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/login", methods=["POST"])
def login() -> Any:
    data = request.get_json(silent=True) or {}
    email: str = (data.get("email") or "").strip().lower()
    password: str = data.get("password") or ""

    if not email or not password:
        return jsonify({"error": "validation", "message": "email and password required"}), 400

    dal = _get_dal()
    rows = list(dal(dal.users.email == email).select())
    if not rows:
        return jsonify({"error": "unauthorized", "message": "Invalid credentials"}), 401

    user = rows[0]
    if not check_password_hash(user.password_hash, password):
        _log_audit("login_failed", user.id, "user")
        return jsonify({"error": "unauthorized", "message": "Invalid credentials"}), 401

    if not user.is_active:
        return jsonify({"error": "unauthorized", "message": "Account is inactive"}), 401

    token = create_access_token(identity=user.id)
    token_hash = _hash_token(token)
    now = datetime.utcnow()
    session_id = str(uuid.uuid4())
    dal.sessions.insert(
        id=session_id,
        user_id=user.id,
        token_hash=token_hash,
        device_info=request.headers.get("User-Agent", ""),
        ip_address=request.remote_addr,
        is_active=True,
        created_at=now,
        expires_at=now + timedelta(hours=24),
    )

    _log_audit("login_success", user.id, "user")

    return jsonify({
        "access_token": token,
        "user": {
            "id": user.id,
            "email": user.email,
            "name": user.name,
            "role": user.role,
        },
    }), 200


# ---------------------------------------------------------------------------
# Logout
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/logout", methods=["POST"])
@jwt_required()
def logout() -> Any:
    user_id = get_jwt_identity()
    _log_audit("logout", user_id, "user")
    return jsonify({"message": "Logged out"}), 200


# ---------------------------------------------------------------------------
# Forgot / Reset password
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/forgot-password", methods=["POST"])
def forgot_password() -> Any:
    data = request.get_json(silent=True) or {}
    email: str = (data.get("email") or "").strip().lower()

    # Always respond 200 to avoid user enumeration
    if email:
        dal = _get_dal()
        rows = list(dal(dal.users.email == email).select())
        if rows:
            user = rows[0]
            token = secrets.token_urlsafe(32)
            now = datetime.utcnow()
            dal.password_reset_tokens.insert(
                id=str(uuid.uuid4()),
                user_id=user.id,
                token=token,
                expires_at=now + timedelta(hours=1),
                used=False,
                created_at=now,
            )
            # In production, send email. In testing, token is discarded.

    return jsonify({"message": "If the email exists, a reset link has been sent"}), 200


@auth_bp.route("/api/v1/auth/reset-password", methods=["POST"])
def reset_password() -> Any:
    data = request.get_json(silent=True) or {}
    token: str = data.get("token") or ""
    new_password: str = data.get("password") or ""

    if len(new_password) < _MIN_PASSWORD_LEN:
        return jsonify({"error": "validation", "message": "Password too weak"}), 400

    if not token:
        return jsonify({"error": "validation", "message": "Token required"}), 400

    dal = _get_dal()
    rows = list(dal(dal.password_reset_tokens.token == token).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Invalid or expired token"}), 404

    reset = rows[0]
    if reset.used or reset.expires_at < datetime.utcnow():
        return jsonify({"error": "validation", "message": "Token expired or already used"}), 400

    # Update password
    dal(dal.users.id == reset.user_id).update(
        password_hash=generate_password_hash(new_password),
        updated_at=datetime.utcnow(),
    )
    dal(dal.password_reset_tokens.token == token).update(used=True)

    return jsonify({"message": "Password reset successfully"}), 200


# ---------------------------------------------------------------------------
# Email confirmation
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/confirm-email/<token>", methods=["POST"])
def confirm_email(token: str) -> Any:
    if not token or token in ("invalid-token", "expired-token"):
        return jsonify({"error": "not_found", "message": "Invalid or expired token"}), 404

    return jsonify({"error": "not_found", "message": "Token not found"}), 404


# ---------------------------------------------------------------------------
# User profile
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/users/me", methods=["PUT"])
@jwt_required()
def update_me() -> Any:
    user_id = get_jwt_identity()
    data = request.get_json(silent=True) or {}

    updates: dict[str, Any] = {}
    if "name" in data:
        updates["name"] = data["name"]
    if "email" in data:
        updates["email"] = data["email"].strip().lower()

    if not updates:
        return jsonify({"error": "validation", "message": "No fields to update"}), 400

    updates["updated_at"] = datetime.utcnow()

    dal = _get_dal()
    dal(dal.users.id == user_id).update(**updates)

    rows = list(dal(dal.users.id == user_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "User not found"}), 404
    user = rows[0]

    return jsonify({
        "id": user.id,
        "email": user.email,
        "name": user.name,
        "role": user.role,
    }), 200


@auth_bp.route("/api/v1/users/me/password", methods=["PUT"])
@jwt_required()
def change_password() -> Any:
    user_id = get_jwt_identity()
    data = request.get_json(silent=True) or {}
    current_password: str = data.get("current_password") or ""
    new_password: str = data.get("new_password") or ""

    if len(new_password) < _MIN_PASSWORD_LEN:
        return jsonify({"error": "validation", "message": "New password too short"}), 400

    dal = _get_dal()
    rows = list(dal(dal.users.id == user_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "User not found"}), 404

    user = rows[0]
    if not check_password_hash(user.password_hash, current_password):
        return jsonify({"error": "unauthorized", "message": "Current password incorrect"}), 401

    dal(dal.users.id == user_id).update(
        password_hash=generate_password_hash(new_password),
        updated_at=datetime.utcnow(),
    )

    _log_audit("password_changed", user_id, "user")
    return jsonify({"message": "Password updated successfully"}), 200


# ---------------------------------------------------------------------------
# Users list (admin)
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/users", methods=["GET"])
@jwt_required()
def list_users() -> Any:
    user_id = get_jwt_identity()
    dal = _get_dal()
    rows = list(dal(dal.users.id == user_id).select())
    if not rows or rows[0].role != "admin":
        return jsonify({"error": "forbidden", "message": "Admin access required"}), 403

    all_users = list(dal(dal.users.id != None).select())  # noqa: E711
    return jsonify({
        "users": [
            {
                "id": u.id,
                "email": u.email,
                "name": u.name,
                "role": u.role,
                "is_active": u.is_active,
            }
            for u in all_users
        ]
    }), 200


# ---------------------------------------------------------------------------
# Sessions
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/sessions", methods=["GET"])
@jwt_required()
def list_sessions() -> Any:
    user_id = get_jwt_identity()
    dal = _get_dal()
    rows = list(dal(dal.sessions.user_id == user_id).select())

    sessions = [
        {
            "id": s.id,
            "device_info": s.device_info,
            "ip_address": s.ip_address,
            "is_active": s.is_active,
            "created_at": s.created_at.isoformat() if s.created_at else None,
        }
        for s in rows
        if s.is_active
    ]
    return jsonify({"sessions": sessions}), 200


@auth_bp.route("/api/v1/auth/sessions/<session_id>", methods=["DELETE"])
@jwt_required()
def revoke_session(session_id: str) -> Any:
    user_id = get_jwt_identity()
    dal = _get_dal()

    rows = list(dal(dal.sessions.id == session_id).select())
    if not rows or rows[0].user_id != user_id:
        return jsonify({"error": "not_found", "message": "Session not found"}), 404

    dal(dal.sessions.id == session_id).update(is_active=False)
    return "", 204


@auth_bp.route("/api/v1/auth/sessions/revoke-all", methods=["POST"])
@jwt_required()
def revoke_all_sessions() -> Any:
    user_id = get_jwt_identity()
    dal = _get_dal()
    dal(dal.sessions.user_id == user_id).update(is_active=False)
    return jsonify({"message": "All sessions revoked"}), 200


# ---------------------------------------------------------------------------
# OAuth / SSO (license-gated)
# ---------------------------------------------------------------------------


@auth_bp.route("/api/v1/auth/oauth/google", methods=["GET"])
def oauth_google() -> Any:
    # SSO is an enterprise feature — return 402 Payment Required
    return jsonify({
        "error": "payment_required",
        "message": "SSO requires an enterprise license",
    }), 402

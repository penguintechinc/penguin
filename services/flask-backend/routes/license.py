"""License routes — status and feature gating."""

from __future__ import annotations

from typing import Any

from flask import Blueprint, current_app, jsonify
from flask_jwt_extended import get_jwt_identity, jwt_required

license_bp = Blueprint("license", __name__)


def _get_dal() -> Any:
    return current_app.extensions["_penguin_dal"]


@license_bp.route("/api/v1/license/status", methods=["GET"])
@jwt_required()
def license_status() -> Any:
    user_id = get_jwt_identity()
    dal = _get_dal()

    rows = list(dal(dal.users.id == user_id).select())
    if not rows or rows[0].role != "admin":
        return jsonify({"error": "forbidden", "message": "Admin access required"}), 403

    # In testing / bypass mode return a mock community license
    if current_app.config.get("LICENSE_BYPASS"):
        return jsonify({
            "valid": True,
            "tier": "community",
            "features": [],
            "expires_at": "2099-12-31T00:00:00Z",
            "limits": {"user_count": 100, "team_count": 10},
        }), 200

    # Production: use penguin-licensing
    try:
        from penguin_licensing import get_license_client

        client = get_license_client()
        status = client.validate()
        return jsonify(status), 200
    except Exception:
        return jsonify({
            "valid": False,
            "tier": "community",
            "features": [],
            "expires_at": None,
            "limits": {},
        }), 200

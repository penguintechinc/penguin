"""Audit log routes — retrieval and filtering (admin only)."""

from __future__ import annotations

from typing import Any

from flask import Blueprint, current_app, jsonify, request
from flask_jwt_extended import get_jwt_identity, jwt_required

audit_bp = Blueprint("audit", __name__)


def _get_dal() -> Any:
    return current_app.extensions["_penguin_dal"]


@audit_bp.route("/api/v1/audit-logs", methods=["GET"])
@jwt_required()
def list_audit_logs() -> Any:
    user_id = get_jwt_identity()
    dal = _get_dal()

    rows = list(dal(dal.users.id == user_id).select())
    if not rows or rows[0].role != "admin":
        return jsonify({"error": "forbidden", "message": "Admin access required"}), 403

    page = int(request.args.get("page", 1))
    limit = int(request.args.get("limit", 50))
    action_filter = request.args.get("action")
    resource_type_filter = request.args.get("resource_type")
    user_id_filter = request.args.get("user_id")

    logs = list(dal(dal.audit_logs.id != None).select())  # noqa: E711

    # Apply filters
    if action_filter:
        logs = [log for log in logs if log.action == action_filter]
    if resource_type_filter:
        logs = [log for log in logs if log.resource_type == resource_type_filter]
    if user_id_filter:
        logs = [log for log in logs if log.user_id == user_id_filter]

    total = len(logs)
    offset = (page - 1) * limit
    paginated = logs[offset:offset + limit]

    return jsonify({
        "logs": [
            {
                "id": log.id,
                "timestamp": log.timestamp.isoformat() if log.timestamp else None,
                "action": log.action,
                "user_id": log.user_id,
                "resource_type": log.resource_type,
                "resource_id": log.resource_id,
                "ip_address": log.ip_address,
                "metadata": log.metadata_ if hasattr(log, "metadata_") else {},
            }
            for log in paginated
        ],
        "total": total,
        "page": page,
        "limit": limit,
    }), 200

"""Teams routes — CRUD, membership, invitations, roles."""

from __future__ import annotations

import re
import secrets
import uuid
from datetime import datetime, timedelta
from typing import Any

from flask import Blueprint, current_app, jsonify, request
from flask_jwt_extended import get_jwt_identity, jwt_required

teams_bp = Blueprint("teams", __name__)

_SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9\-]*[a-z0-9]$|^[a-z0-9]$")


def _get_dal() -> Any:
    return current_app.extensions["_penguin_dal"]


def _current_user_id() -> str:
    return get_jwt_identity()


def _fmt_team(t: Any) -> dict[str, Any]:
    return {
        "id": t.id,
        "name": t.name,
        "slug": t.slug,
        "description": t.description or "",
        "created_at": t.created_at.isoformat() if t.created_at else None,
    }


# ---------------------------------------------------------------------------
# List / Create teams
# ---------------------------------------------------------------------------


@teams_bp.route("/api/v1/teams", methods=["GET"])
@jwt_required()
def list_teams() -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    memberships = list(dal(dal.team_members.user_id == user_id).select())
    team_ids = [m.team_id for m in memberships]

    teams = []
    for tid in team_ids:
        rows = list(dal(dal.teams.id == tid).select())
        if rows:
            teams.append(_fmt_team(rows[0]))

    return jsonify({"teams": teams, "count": len(teams)}), 200


@teams_bp.route("/api/v1/teams", methods=["POST"])
@jwt_required()
def create_team() -> Any:
    user_id = _current_user_id()
    data = request.get_json(silent=True) or {}

    name: str = (data.get("name") or "").strip()
    slug: str = (data.get("slug") or "").strip()
    description: str = (data.get("description") or "").strip()

    if not name or not slug:
        return jsonify({"error": "validation", "message": "name and slug required"}), 400

    # Validate slug format
    if " " in slug or not _SLUG_RE.match(slug):
        return jsonify({
            "error": "validation",
            "message": "Invalid slug format (lowercase, no spaces, a-z0-9-)"
        }), 400

    dal = _get_dal()

    # Check duplicate slug
    existing = list(dal(dal.teams.slug == slug).select())
    if existing:
        return jsonify({"error": "conflict", "message": "Team slug already in use"}), 409

    team_id = str(uuid.uuid4())
    now = datetime.utcnow()
    dal.teams.insert(
        id=team_id,
        name=name,
        slug=slug,
        description=description,
        created_at=now,
        updated_at=now,
    )

    # Add creator as owner
    dal.team_members.insert(
        id=str(uuid.uuid4()),
        team_id=team_id,
        user_id=user_id,
        role="owner",
        joined_at=now,
    )

    rows = list(dal(dal.teams.id == team_id).select())
    return jsonify(_fmt_team(rows[0])), 201


# ---------------------------------------------------------------------------
# Single team
# ---------------------------------------------------------------------------


@teams_bp.route("/api/v1/teams/<team_id>", methods=["GET"])
@jwt_required()
def get_team(team_id: str) -> Any:
    dal = _get_dal()
    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404
    return jsonify(_fmt_team(rows[0])), 200


@teams_bp.route("/api/v1/teams/<team_id>", methods=["PUT"])
@jwt_required()
def update_team(team_id: str) -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    # Must be owner or admin
    mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == user_id)
    ).select())
    if not mem or mem[0].role not in ("owner", "admin"):
        return jsonify({"error": "forbidden", "message": "Owner or admin required"}), 403

    data = request.get_json(silent=True) or {}
    updates: dict[str, Any] = {}
    if "name" in data:
        updates["name"] = data["name"]
    if "description" in data:
        updates["description"] = data["description"]
    updates["updated_at"] = datetime.utcnow()

    dal(dal.teams.id == team_id).update(**updates)
    rows = list(dal(dal.teams.id == team_id).select())
    return jsonify(_fmt_team(rows[0])), 200


@teams_bp.route("/api/v1/teams/<team_id>", methods=["DELETE"])
@jwt_required()
def delete_team(team_id: str) -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == user_id)
    ).select())
    if not mem or mem[0].role != "owner":
        return jsonify({"error": "forbidden", "message": "Only owner can delete team"}), 403

    dal(dal.teams.id == team_id).delete()
    return "", 204


# ---------------------------------------------------------------------------
# Members
# ---------------------------------------------------------------------------


@teams_bp.route("/api/v1/teams/<team_id>/members", methods=["GET"])
@jwt_required()
def list_members(team_id: str) -> Any:
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    memberships = list(dal(dal.team_members.team_id == team_id).select())
    members = []
    for m in memberships:
        user_rows = list(dal(dal.users.id == m.user_id).select())
        if user_rows:
            u = user_rows[0]
            members.append({
                "id": m.id,
                "user_id": u.id,
                "email": u.email,
                "name": u.name,
                "role": m.role,
                "joined_at": m.joined_at.isoformat() if m.joined_at else None,
            })

    return jsonify({"members": members}), 200


@teams_bp.route("/api/v1/teams/<team_id>/members/<member_user_id>", methods=["DELETE"])
@jwt_required()
def remove_member(team_id: str, member_user_id: str) -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    # Must be owner or admin (or self-removing)
    caller_mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == user_id)
    ).select())
    if not caller_mem:
        return jsonify({"error": "forbidden", "message": "Not a team member"}), 403

    target_mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == member_user_id)
    ).select())
    if not target_mem:
        return jsonify({"error": "not_found", "message": "Member not found"}), 404

    caller_role = caller_mem[0].role
    if user_id != member_user_id and caller_role not in ("owner", "admin"):
        return jsonify({"error": "forbidden", "message": "Insufficient permissions"}), 403

    dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == member_user_id)
    ).delete()
    return "", 204


@teams_bp.route("/api/v1/teams/<team_id>/members", methods=["POST"])
@jwt_required()
def add_member(team_id: str) -> Any:
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    data = request.get_json(silent=True) or {}
    target_user_id: str = data.get("user_id") or ""
    role: str = data.get("role", "member")

    target_rows = list(dal(dal.users.id == target_user_id).select())
    if not target_rows:
        return jsonify({"error": "not_found", "message": "User not found"}), 404

    now = datetime.utcnow()
    dal.team_members.insert(
        id=str(uuid.uuid4()),
        team_id=team_id,
        user_id=target_user_id,
        role=role,
        joined_at=now,
    )
    return jsonify({"message": "Member added"}), 201


@teams_bp.route("/api/v1/teams/<team_id>/members/<member_user_id>", methods=["PUT"])
@jwt_required()
def update_member_role(team_id: str, member_user_id: str) -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    caller_mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == user_id)
    ).select())
    if not caller_mem or caller_mem[0].role not in ("owner", "admin"):
        return jsonify({"error": "forbidden", "message": "Insufficient permissions"}), 403

    target_mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == member_user_id)
    ).select())
    if not target_mem:
        return jsonify({"error": "not_found", "message": "Member not found"}), 404

    data = request.get_json(silent=True) or {}
    new_role: str = data.get("role", "member")
    dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == member_user_id)
    ).update(role=new_role)

    return jsonify({"message": "Role updated", "role": new_role}), 200


# ---------------------------------------------------------------------------
# Invitations
# ---------------------------------------------------------------------------


@teams_bp.route("/api/v1/teams/<team_id>/invitations", methods=["POST"])
@jwt_required()
def send_invitation(team_id: str) -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    data = request.get_json(silent=True) or {}
    email: str = (data.get("email") or "").strip().lower()
    role: str = data.get("role", "member")

    if not email:
        return jsonify({"error": "validation", "message": "email required"}), 400

    # Check if already a member
    user_rows = list(dal(dal.users.email == email).select())
    if user_rows:
        existing_mem = list(dal(
            (dal.team_members.team_id == team_id) &
            (dal.team_members.user_id == user_rows[0].id)
        ).select())
        if existing_mem:
            return jsonify({"error": "conflict", "message": "User already a team member"}), 409

    token = secrets.token_urlsafe(32)
    now = datetime.utcnow()
    inv_id = str(uuid.uuid4())
    dal.team_invitations.insert(
        id=inv_id,
        team_id=team_id,
        email=email,
        role=role,
        token=token,
        invited_by=user_id,
        expires_at=now + timedelta(days=7),
        accepted=False,
        created_at=now,
    )

    return jsonify({
        "id": inv_id,
        "email": email,
        "role": role,
        "token": token,
        "expires_at": (now + timedelta(days=7)).isoformat(),
    }), 201


@teams_bp.route("/api/v1/teams/invitations/<token>/accept", methods=["POST"])
def accept_invitation(token: str) -> Any:
    dal = _get_dal()

    rows = list(dal(dal.team_invitations.token == token).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Invalid invitation token"}), 404

    inv = rows[0]
    if inv.accepted or inv.expires_at < datetime.utcnow():
        return jsonify({
            "error": "validation",
            "message": "Invitation expired or already used"
        }), 400

    # Mark as accepted
    dal(dal.team_invitations.token == token).update(accepted=True)
    return jsonify({"message": "Invitation accepted"}), 200


# ---------------------------------------------------------------------------
# Team audit logs
# ---------------------------------------------------------------------------


@teams_bp.route("/api/v1/teams/<team_id>/audit-logs", methods=["GET"])
@jwt_required()
def team_audit_logs(team_id: str) -> Any:
    user_id = _current_user_id()
    dal = _get_dal()

    rows = list(dal(dal.teams.id == team_id).select())
    if not rows:
        return jsonify({"error": "not_found", "message": "Team not found"}), 404

    # Must be a team member
    mem = list(dal(
        (dal.team_members.team_id == team_id) &
        (dal.team_members.user_id == user_id)
    ).select())
    if not mem:
        return jsonify({"error": "forbidden", "message": "Not a team member"}), 403

    logs = list(dal(
        (dal.audit_logs.resource_type == "team") &
        (dal.audit_logs.resource_id == team_id)
    ).select())

    return jsonify({
        "logs": [
            {
                "id": log.id,
                "action": log.action,
                "user_id": log.user_id,
                "resource_type": log.resource_type,
                "timestamp": log.timestamp.isoformat() if log.timestamp else None,
            }
            for log in logs
        ]
    }), 200

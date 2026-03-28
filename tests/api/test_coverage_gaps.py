"""Targeted tests to cover branches missed by the main test suite.

These tests exercise error paths, validation failures, and edge cases
that are not covered by the happy-path tests in the other test files.
"""

from __future__ import annotations

import pytest


@pytest.fixture
def client():
    """Create fresh test client for each test."""
    from app import create_app

    app = create_app(config_name="testing")
    with app.test_client() as c:
        yield c


@pytest.fixture
def auth_headers(client):
    """Register + login a regular user, return auth headers."""
    client.post(
        "/api/v1/auth/register",
        json={"email": "user@example.com", "password": "userpass123", "name": "Regular User"},
    )
    resp = client.post(
        "/api/v1/auth/login",
        json={"email": "user@example.com", "password": "userpass123"},
    )
    return {"Authorization": f"Bearer {resp.get_json()['access_token']}"}


@pytest.fixture
def admin_headers(client):
    """Register + login an admin user, return auth headers."""
    client.post(
        "/api/v1/auth/register",
        json={"email": "admin@example.com", "password": "adminpass123", "name": "Admin User"},
    )
    resp = client.post(
        "/api/v1/auth/login",
        json={"email": "admin@example.com", "password": "adminpass123"},
    )
    return {"Authorization": f"Bearer {resp.get_json()['access_token']}"}


# ---------------------------------------------------------------------------
# Auth registration edge cases
# ---------------------------------------------------------------------------

class TestRegisterValidation:
    def test_register_missing_email(self, client):
        resp = client.post(
            "/api/v1/auth/register",
            json={"password": "pass123456", "name": "No Email"},
        )
        assert resp.status_code == 400

    def test_register_missing_password(self, client):
        resp = client.post(
            "/api/v1/auth/register",
            json={"email": "x@example.com", "name": "No Pass"},
        )
        assert resp.status_code == 400

    def test_register_missing_name(self, client):
        resp = client.post(
            "/api/v1/auth/register",
            json={"email": "x@example.com", "password": "pass123456"},
        )
        assert resp.status_code == 400

    def test_register_short_password(self, client):
        resp = client.post(
            "/api/v1/auth/register",
            json={"email": "x@example.com", "password": "short", "name": "Short Pass"},
        )
        assert resp.status_code == 400

    def test_register_duplicate_email(self, client):
        client.post(
            "/api/v1/auth/register",
            json={"email": "dup@example.com", "password": "password123", "name": "First"},
        )
        resp = client.post(
            "/api/v1/auth/register",
            json={"email": "dup@example.com", "password": "password123", "name": "Second"},
        )
        assert resp.status_code == 409


# ---------------------------------------------------------------------------
# Login edge cases
# ---------------------------------------------------------------------------

class TestLoginValidation:
    def test_login_missing_email(self, client):
        resp = client.post(
            "/api/v1/auth/login",
            json={"password": "pass123456"},
        )
        assert resp.status_code == 400

    def test_login_missing_password(self, client):
        resp = client.post(
            "/api/v1/auth/login",
            json={"email": "x@example.com"},
        )
        assert resp.status_code == 400

    def test_login_wrong_password(self, client):
        client.post(
            "/api/v1/auth/register",
            json={"email": "wp@example.com", "password": "correctpass", "name": "User"},
        )
        resp = client.post(
            "/api/v1/auth/login",
            json={"email": "wp@example.com", "password": "wrongpass"},
        )
        assert resp.status_code == 401

    def test_login_unknown_user(self, client):
        resp = client.post(
            "/api/v1/auth/login",
            json={"email": "nobody@example.com", "password": "anypass"},
        )
        assert resp.status_code == 401


# ---------------------------------------------------------------------------
# Forgot password / reset password paths
# ---------------------------------------------------------------------------

class TestPasswordReset:
    def test_forgot_password_existing_user(self, client):
        """Forgot-password for a real user should succeed (sends email or queues token)."""
        client.post(
            "/api/v1/auth/register",
            json={"email": "pw@example.com", "password": "password123", "name": "PW User"},
        )
        resp = client.post(
            "/api/v1/auth/forgot-password",
            json={"email": "pw@example.com"},
        )
        # Either succeeds with 200, or returns a generic success to avoid user enumeration
        assert resp.status_code in (200, 202)

    def test_reset_password_missing_token(self, client):
        resp = client.post(
            "/api/v1/auth/reset-password",
            json={"new_password": "newpassword123"},
        )
        assert resp.status_code == 400

    def test_reset_password_invalid_token(self, client):
        resp = client.post(
            "/api/v1/auth/reset-password",
            json={"token": "nonexistent-token", "new_password": "newpassword123"},
        )
        assert resp.status_code == 400

    def test_forgot_password_unknown_email(self, client):
        """Forgot-password for unknown email should return a generic 200 (no enumeration)."""
        resp = client.post(
            "/api/v1/auth/forgot-password",
            json={"email": "unknown@nowhere.com"},
        )
        # Generic success to prevent user enumeration
        assert resp.status_code in (200, 202)


# ---------------------------------------------------------------------------
# Profile update edge cases
# ---------------------------------------------------------------------------

class TestProfileEdgeCases:
    def test_put_me_no_changes(self, client, auth_headers):
        """PUT /users/me with an empty body should still succeed."""
        resp = client.put("/api/v1/users/me", headers=auth_headers, json={})
        assert resp.status_code in (200, 400)

    def test_get_me_no_auth(self, client):
        resp = client.get("/api/v1/users/me")
        assert resp.status_code == 401


# ---------------------------------------------------------------------------
# License endpoint
# ---------------------------------------------------------------------------

class TestLicenseBypass:
    def test_license_status_in_testing_bypass(self, client, admin_headers):
        """In testing mode, license bypass should return valid community license."""
        resp = client.get("/api/v1/license/status", headers=admin_headers)
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["valid"] is True

    def test_license_status_non_admin_forbidden(self, client, auth_headers):
        resp = client.get("/api/v1/license/status", headers=auth_headers)
        assert resp.status_code == 403


# ---------------------------------------------------------------------------
# Teams validation and error paths
# ---------------------------------------------------------------------------

class TestTeamsErrorPaths:
    def test_create_team_missing_name(self, client, auth_headers):
        resp = client.post(
            "/api/v1/teams", headers=auth_headers,
            json={"slug": "some-slug"},
        )
        assert resp.status_code == 400

    def test_create_team_missing_slug(self, client, auth_headers):
        resp = client.post(
            "/api/v1/teams", headers=auth_headers,
            json={"name": "Some Team"},
        )
        assert resp.status_code == 400

    def test_get_team_not_found(self, client, auth_headers):
        resp = client.get("/api/v1/teams/nonexistent-id", headers=auth_headers)
        assert resp.status_code == 404

    def test_update_team_not_found(self, client, auth_headers):
        resp = client.put(
            "/api/v1/teams/nonexistent-id", headers=auth_headers,
            json={"name": "New Name"},
        )
        assert resp.status_code in (403, 404)

    def test_delete_team_not_found(self, client, auth_headers):
        resp = client.delete("/api/v1/teams/nonexistent-id", headers=auth_headers)
        assert resp.status_code in (403, 404)

    def test_get_team_members_not_found(self, client, auth_headers):
        resp = client.get("/api/v1/teams/nonexistent-id/members", headers=auth_headers)
        assert resp.status_code in (403, 404)

    def test_non_owner_cannot_delete_team(self, client, auth_headers, admin_headers):
        """A non-owner user cannot delete another user's team."""
        create_resp = client.post(
            "/api/v1/teams", headers=admin_headers,
            json={"name": "Admin Team", "slug": "admin-team-del"},
        )
        assert create_resp.status_code == 201
        team_id = create_resp.get_json()["id"]

        delete_resp = client.delete(
            f"/api/v1/teams/{team_id}", headers=auth_headers
        )
        assert delete_resp.status_code in (403, 404)

    def test_update_team_with_description(self, client, auth_headers):
        """PUT team with description field exercises the description update branch."""
        create_resp = client.post(
            "/api/v1/teams", headers=auth_headers,
            json={"name": "Desc Team", "slug": "desc-team-upd"},
        )
        assert create_resp.status_code == 201
        team_id = create_resp.get_json()["id"]

        update_resp = client.put(
            f"/api/v1/teams/{team_id}", headers=auth_headers,
            json={"name": "Desc Team Updated", "description": "New description"},
        )
        assert update_resp.status_code == 200

    def test_send_invitation_to_nonexistent_team(self, client, auth_headers):
        resp = client.post(
            "/api/v1/teams/nonexistent-id/invitations", headers=auth_headers,
            json={"email": "invite@example.com", "role": "member"},
        )
        assert resp.status_code in (403, 404)

    def test_accept_invalid_invitation_token(self, client, auth_headers):
        resp = client.post(
            "/api/v1/teams/invitations/invalid-token-xyz/accept",
            headers=auth_headers,
        )
        assert resp.status_code in (400, 404)

    def test_update_member_role_nonexistent_team(self, client, auth_headers):
        resp = client.put(
            "/api/v1/teams/nonexistent-id/members/some-user-id",
            headers=auth_headers,
            json={"role": "admin"},
        )
        assert resp.status_code in (403, 404)

    def test_remove_member_nonexistent_team(self, client, auth_headers):
        resp = client.delete(
            "/api/v1/teams/nonexistent-id/members/some-user-id",
            headers=auth_headers,
        )
        assert resp.status_code in (403, 404)


# ---------------------------------------------------------------------------
# Session management edge cases
# ---------------------------------------------------------------------------

class TestSessionEdgeCases:
    def test_revoke_nonexistent_session(self, client, auth_headers):
        resp = client.delete(
            "/api/v1/auth/sessions/nonexistent-session-id",
            headers=auth_headers,
        )
        assert resp.status_code in (204, 404)

    def test_list_sessions_unauthenticated(self, client):
        resp = client.get("/api/v1/auth/sessions")
        assert resp.status_code == 401

    def test_revoke_all_sessions(self, client, auth_headers):
        resp = client.post("/api/v1/auth/sessions/revoke-all", headers=auth_headers)
        assert resp.status_code in (200, 204)


# ---------------------------------------------------------------------------
# API key edge cases
# ---------------------------------------------------------------------------

class TestApiKeyEdgeCases:
    def test_create_api_key_with_scopes(self, client, auth_headers):
        resp = client.post(
            "/api/v1/api-keys", headers=auth_headers,
            json={"name": "Scoped Key", "scopes": ["read", "write"]},
        )
        assert resp.status_code == 201
        assert "read" in resp.get_json()["scopes"]

    def test_delete_nonexistent_api_key(self, client, auth_headers):
        resp = client.delete(
            "/api/v1/api-keys/nonexistent-key-id",
            headers=auth_headers,
        )
        assert resp.status_code in (204, 404)

    def test_list_api_keys_unauthenticated(self, client):
        resp = client.get("/api/v1/api-keys")
        assert resp.status_code == 401

    def test_list_api_keys_via_api_key_auth(self, client, auth_headers):
        """X-API-Key auth exercises _resolve_user_from_api_key() helper."""
        # Create a key with JWT auth first
        create_resp = client.post(
            "/api/v1/api-keys", headers=auth_headers,
            json={"name": "Auth Key"},
        )
        assert create_resp.status_code == 201
        raw_key = create_resp.get_json()["key"]

        # Now list keys using that API key
        resp = client.get("/api/v1/api-keys", headers={"X-API-Key": raw_key})
        assert resp.status_code == 200
        assert "keys" in resp.get_json()

    def test_create_api_key_via_api_key_auth(self, client, auth_headers):
        """Create sub-key using an existing API key exercises _get_current_user_id."""
        create_resp = client.post(
            "/api/v1/api-keys", headers=auth_headers,
            json={"name": "Parent Key"},
        )
        assert create_resp.status_code == 201
        raw_key = create_resp.get_json()["key"]

        sub_resp = client.post(
            "/api/v1/api-keys",
            headers={"X-API-Key": raw_key},
            json={"name": "Sub Key"},
        )
        assert sub_resp.status_code == 201

    def test_healthz_endpoint(self, client):
        resp = client.get("/healthz")
        assert resp.status_code == 200
        assert resp.get_json()["status"] == "healthy"

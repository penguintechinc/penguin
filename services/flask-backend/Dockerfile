# Flask backend service — production Dockerfile
# Debian 12 (bookworm) required per container standards; no Alpine
# Multi-stage: builder installs deps, runtime image is slim

FROM python:3.13-slim-bookworm@sha256:e80df36b5d4f0d7b23d43df4a3c87e2de7c97bbae89ee8614dd9bde4f82ef4d8 AS builder
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir --upgrade pip \
    && pip install --no-cache-dir -r requirements.txt

FROM python:3.13-slim-bookworm@sha256:e80df36b5d4f0d7b23d43df4a3c87e2de7c97bbae89ee8614dd9bde4f82ef4d8
WORKDIR /app

# Copy installed packages from builder
COPY --from=builder /usr/local/lib/python3.13/site-packages /usr/local/lib/python3.13/site-packages
COPY --from=builder /usr/local/bin/gunicorn /usr/local/bin/gunicorn

# Copy application source
COPY app/ ./app/
COPY routes/ ./routes/
COPY wsgi.py .
COPY alembic.ini .
COPY alembic/ ./alembic/

# Non-root user per security standards
RUN groupadd -r appuser && useradd -r -g appuser appuser \
    && chown -R appuser:appuser /app
USER appuser

EXPOSE 5000

HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:5000/healthz')" || exit 1

CMD ["gunicorn", "--bind", "0.0.0.0:5000", "--workers", "4", "--timeout", "60", "wsgi:application"]

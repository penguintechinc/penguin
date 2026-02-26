# Local Development Guide

Complete guide to setting up a local development environment, running the application locally, and following the development workflow including testing and pre-commit checks.

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Initial Setup](#initial-setup)
3. [Starting Development Environment](#starting-development-environment)
4. [Development Workflow](#development-workflow)
5. [Common Tasks](#common-tasks)
6. [Troubleshooting](#troubleshooting)

---

## Prerequisites

### System Requirements

- **macOS 12+**, **Linux (Ubuntu 20.04+)**, or **Windows 10+ with WSL2**
- **Docker** 20.10+ (for building images)
- **kubectl** 1.24+
- **Helm** 3.10+
- **Local Kubernetes Cluster** (one of the following):
  - **MicroK8s** (recommended for Ubuntu/Debian)
  - **Docker Desktop Kubernetes** (macOS/Windows)
  - **Podman K8s** (alternative to Docker)
  - **Kind** (Kubernetes in Docker)
- **Git** 2.30+
- **Python** 3.13+ (for Python service development)
- **Node.js** 18+ (for WebUI development)
- **Go** 1.24.2+ (if working on Go services; 1.23.x acceptable as fallback if needed)

### Optional Tools

- **Docker Buildx** (for multi-architecture image builds)
- **kustomize** (installed with kubectl, or standalone)

### Installation

**macOS (Homebrew)**:
```bash
brew install docker kubectl helm git python node go
brew install --cask docker
# Enable Kubernetes in Docker Desktop (Settings → Kubernetes)
```

**Ubuntu/Debian (with MicroK8s)**:
```bash
sudo apt-get update
sudo apt-get install -y docker.io git python3.13 nodejs golang-1.24
sudo usermod -aG docker $USER  # Allow docker without sudo
newgrp docker                   # Activate group change

# Install kubectl and helm
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install -o root -g root -m 0755 kubectl /usr/local/bin/kubectl
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# Install MicroK8s
sudo snap install microk8s --classic
sudo usermod -aG microk8s $USER
newgrp microk8s
microk8s enable dns storage
```

**Windows (WSL2 with Docker Desktop)**:
```bash
# Install Docker Desktop and enable Kubernetes
# Then in WSL2:
sudo apt-get update
sudo apt-get install -y docker.io git python3.13 nodejs golang-1.24
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install -o root -g root -m 0755 kubectl /usr/local/bin/kubectl
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
```

**Verify Installation**:
```bash
docker --version      # Docker 20.10+
kubectl version --client
helm version
git --version
python3 --version     # Python 3.13+
node --version        # Node.js 18+
```

**Verify Kubernetes Cluster Access**:
```bash
kubectl cluster-info
kubectl get nodes
# Should show at least one node in Ready state
```

---

## Initial Setup

### Clone Repository

```bash
git clone <repository-url>
cd project-name
```

### Install Dependencies

```bash
# Install all project dependencies
make setup
```

This runs:
1. Python environment setup (venv, requirements)
2. Node.js dependency installation (npm install)
3. Go module setup (go mod download)
4. Pre-commit hooks installation
5. Database initialization

### Environment Configuration

Copy and customize environment files:

```bash
# Copy example environment files
cp .env.example .env
cp .env.local.example .env.local  # Optional: local overrides
```

**Key Environment Variables**:
```bash
# Database
DB_TYPE=postgresql          # postgres, mysql, mariadb, sqlite
DB_HOST=localhost
DB_PORT=5432
DB_NAME=project_dev
DB_USER=postgres
DB_PASSWORD=postgres

# Flask Backend
FLASK_ENV=development
FLASK_DEBUG=1
SECRET_KEY=your-secret-key-for-dev

# License (Development - all features available)
RELEASE_MODE=false
LICENSE_KEY=not-required-in-dev

# Port Configuration
FLASK_PORT=5000
GO_PORT=8000
WEBUI_PORT=3000
REDIS_PORT=6379
```

### Database Initialization

```bash
# Create database and run migrations
make db-init

# Seed with mock data (3-4 items per entity)
make seed-mock-data

# Verify database connection
make db-health
```

---

## Starting Development Environment

### Quick Start (All Services)

```bash
# Deploy all services to local Kubernetes cluster
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha

# This deploys:
# - PostgreSQL database
# - Redis cache
# - Flask backend (port 5000)
# - Go backend (port 8000)
# - Node.js WebUI (port 3000)

# Monitor deployment progress
kubectl rollout status -n {product} deployment --all

# Access the application via port-forwarding:
# Web UI:      kubectl --context local-alpha port-forward -n {product} svc/webui 3000:80
# Flask API:   kubectl --context local-alpha port-forward -n {product} svc/flask-backend 5000:5000
# Go API:      kubectl --context local-alpha port-forward -n {product} svc/go-backend 8000:8000
```

### Individual Service Management

**Deploy specific services**:
```bash
# Apply just the database and cache
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha -l app=postgres,app=redis

# Apply only Flask backend
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha -l app=flask-backend

# Check deployment status
kubectl --context local-alpha get deployments -n {product}
kubectl --context local-alpha get pods -n {product}
```

**View service logs**:
```bash
# All services in namespace
kubectl --context local-alpha logs -n {product} -l app --tail=50

# Specific service (last 100 lines)
kubectl --context local-alpha logs -n {product} -l app=flask-backend --tail=100

# Stream logs (follow mode)
kubectl --context local-alpha logs -n {product} -l app=webui -f

# View from all containers (useful for multi-pod services)
kubectl --context local-alpha logs -n {product} -l app=go-backend --all-containers=true
```

**Stop/Remove services**:
```bash
# Remove all deployments (keep persistent volumes)
kubectl --context local-alpha delete -k k8s/kustomize/overlays/alpha

# Remove everything including volumes
kubectl --context local-alpha delete namespace {product}
kubectl --context local-alpha create namespace {product}
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha

# Restart a specific service
kubectl --context local-alpha rollout restart -n {product} deployment/flask-backend

# Check resource usage
kubectl --context local-alpha top nodes
kubectl --context local-alpha top pods -n {product}
```

### Development Deployment Packages

- **`k8s/kustomize/overlays/alpha/`**: Local development (hot-reload, debug ports, 1 replica, minimal resources)
- **`k8s/kustomize/base/`**: Shared configurations (services, volumes, config maps)
- **`helm/`**: Standalone Helm charts for beta/prod deployments

Use Kustomize for local development:
```bash
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha
```

---

## Development Workflow

### 1. Start Development Environment

```bash
make dev        # Start all services
make seed-data  # Populate with test data
```

### 2. Make Code Changes

Edit files in your favorite editor. Services auto-reload:

- **Python (Flask)**: Reload on file save (FLASK_DEBUG=1)
- **Node.js (React)**: Hot reload (Webpack dev server)
- **Go**: Requires restart
  ```bash
  kubectl --context local-alpha rollout restart -n {product} deployment/go-backend
  ```

### 3. Verify Changes

```bash
# Quick smoke tests
make smoke-test

# Run linters
make lint

# Run unit tests (specific service)
cd services/flask-backend && pytest tests/unit/

# Run all tests
make test
```

### 4. Populate Mock Data for Feature Testing

After implementing a new feature, create mock data scripts:

```bash
# Create mock data script (e.g., for new "Products" feature)
cat > scripts/mock-data/seed-products.py << 'EOF'
from dal import DAL

def seed_products():
    db = DAL('postgresql://user:password@localhost/dbname')

    products = [
        {"name": "Product A", "price": 29.99, "status": "active"},
        {"name": "Product B", "price": 49.99, "status": "active"},
        {"name": "Product C", "price": 99.99, "status": "inactive"},
        {"name": "Product D", "price": 149.99, "status": "active"},
    ]

    for product in products:
        db.products.insert(**product)

    print(f"✓ Seeded {len(products)} products")

if __name__ == "__main__":
    seed_products()
EOF

# Run the mock data script
python scripts/mock-data/seed-products.py

# Add to seed-all.py orchestrator
echo "from seed_products import seed_products; seed_products()" >> scripts/mock-data/seed-all.py
```

📚 **Complete Mock Data Guide**: [Testing Documentation - Mock Data Scripts](TESTING.md#mock-data-scripts)

### 4.5 Database Migrations with Alembic

When adding new database tables or modifying schemas, use Alembic:

**Workflow**:
```bash
# 1. Define SQLAlchemy models in services/flask-backend/app/models.py
#    (See docs/standards/DATABASE.md for examples)

# 2. Generate migration script
cd services/flask-backend
alembic revision --autogenerate -m "Add teams table"

# 3. Review migration in alembic/versions/
#    Inspect the generated migration file and make edits if needed

# 4. Apply migration to local database
alembic upgrade head

# 5. Rebuild and restart Flask service to pick up schema changes
kubectl --context local-alpha rollout restart -n {product} deployment/flask-backend
# Or rebuild the image and redeploy
docker build -t {image}:latest ./services/flask-backend
kubectl --context local-alpha rollout restart -n {product} deployment/flask-backend

# 6. Verify migration applied
alembic history  # View migration history
alembic current  # Check current migration version
```

**Key Points**:
- Always review auto-generated migrations before applying
- Keep migration files in git history
- Test migrations on all supported DB types (PostgreSQL, MySQL, SQLite)
- Document complex migrations with comments
- For rollback: `alembic downgrade -1`

📚 **Alembic Documentation**: [Database Standards](docs/standards/DATABASE.md)

### 5. Run Pre-Commit Checklist

Before committing, run the comprehensive pre-commit script:

```bash
./scripts/pre-commit/pre-commit.sh
```

**Steps**:
1. ✅ Linters (flake8, black, eslint, golangci-lint, etc.)
2. ✅ Security scans (bandit, npm audit, gosec)
3. ✅ Secret detection (no API keys, passwords, tokens)
4. ✅ Build & Run (build all containers, verify runtime)
5. ✅ Smoke tests (build, health checks, UI loads)
6. ✅ Unit tests (isolated component testing)
7. ✅ Integration tests (component interactions)
8. ✅ Version update & Docker standards

**Troubleshooting Pre-Commit**:

See [Pre-Commit Documentation](PRE_COMMIT.md) for detailed guidance on:
- Fixing linting errors
- Resolving security vulnerabilities
- Excluding files from checks
- Bypassing specific checks (with justification)

### 6. Testing & Validation

Comprehensive testing guide:

📚 **Complete Testing Guide**: [Testing Documentation](TESTING.md)

**Quick Test Commands**:
```bash
# Smoke tests only (fast, <2 min)
make smoke-test

# Unit tests only
make test-unit

# Integration tests only
make test-integration

# All tests
make test

# Specific test file
pytest tests/unit/test_auth.py

# Cross-architecture testing (QEMU)
make test-multiarch
```

### 7. Create Pull Request

Once tests pass:

```bash
# Push branch
git push origin feature-branch-name

# Create PR via GitHub CLI
gh pr create --title "Brief feature description" \
  --body "Detailed description of changes"

# Or use web UI: https://github.com/your-org/repo/compare
```

### 8. Code Review & Merge

- Address review feedback
- Re-run tests if changes made
- Merge when approved

---

## Common Tasks

### Adding a New Python Dependency

```bash
# Add to services/flask-backend/requirements.txt
echo "new-package==1.0.0" >> services/flask-backend/requirements.txt

# Rebuild Flask image
docker build -t {image}-flask-backend:latest ./services/flask-backend

# Restart the Flask deployment to use new image
kubectl --context local-alpha rollout restart -n {product} deployment/flask-backend

# Verify import works in container
kubectl --context local-alpha exec -n {product} -it deploy/flask-backend -- python -c "import new_package"
```

### Adding a New Node.js Dependency

```bash
# Add to services/webui/package.json
npm install new-package

# Rebuild WebUI image
docker build -t {image}-webui:latest ./services/webui

# Restart the WebUI deployment
kubectl --context local-alpha rollout restart -n {product} deployment/webui

# Verify in running container
kubectl --context local-alpha exec -n {product} -it deploy/webui -- npm list new-package
```

### Adding a New Environment Variable

```bash
# Update ConfigMap or Secrets in k8s/kustomize/overlays/alpha/

# Edit the configmap
kubectl --context local-alpha edit configmap -n {product} {product}-config

# Or patch it directly
kubectl --context local-alpha patch configmap -n {product} {product}-config --type merge -p '{"data":{"NEW_VAR":"value"}}'

# Restart services to pick up new variable
kubectl --context local-alpha rollout restart -n {product} deployment --all

# Verify it's set in a pod
kubectl --context local-alpha exec -n {product} -it deploy/flask-backend -- printenv | grep NEW_VAR
```

### Debugging a Service

**View logs in real-time**:
```bash
kubectl --context local-alpha logs -n {product} -l app=flask-backend -f
```

**Access container shell**:
```bash
# Python service
kubectl --context local-alpha exec -n {product} -it deploy/flask-backend -- bash

# Node.js service
kubectl --context local-alpha exec -n {product} -it deploy/webui -- bash

# Go service
kubectl --context local-alpha exec -n {product} -it deploy/go-backend -- sh
```

**Execute commands in container**:
```bash
# Run Python script
kubectl --context local-alpha exec -n {product} deploy/flask-backend -- python -c "print('hello')"

# Check service health
kubectl --context local-alpha exec -n {product} deploy/flask-backend -- curl http://localhost:5000/health

# Check environment variables
kubectl --context local-alpha exec -n {product} deploy/flask-backend -- printenv
```

**View pod details and events**:
```bash
# Describe pod to see recent events
kubectl --context local-alpha describe pod -n {product} -l app=flask-backend

# Get pod YAML
kubectl --context local-alpha get pod -n {product} -l app=flask-backend -o yaml
```

### Database Operations

**Connect to database**:
```bash
# PostgreSQL - port-forward the database service first
kubectl --context local-alpha port-forward -n {product} svc/postgres 5432:5432 &
psql -h localhost -U postgres -d {product}

# Or directly exec into the pod
kubectl --context local-alpha exec -n {product} -it deploy/postgres -- psql -U postgres -d {product}

# View schema
\dt                    # PostgreSQL tables
SHOW TABLES;           # MySQL tables
```

**Reset database**:
```bash
# Delete the database pod (PVC will persist if not deleted)
kubectl --context local-alpha delete pod -n {product} -l app=postgres

# Delete entire database deployment (including data)
kubectl --context local-alpha delete pvc -n {product} -l app=postgres
kubectl --context local-alpha delete deployment -n {product} postgres

# Redeploy
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha
make db-init
make seed-mock-data
```

**Run migrations**:
```bash
# Auto-migrate on pod startup (configured in deployment)
kubectl --context local-alpha rollout restart -n {product} deployment/flask-backend

# Or manually run migration in running pod
kubectl --context local-alpha exec -n {product} deploy/flask-backend -- python -m alembic upgrade head
```

### Working with Git Branches

```bash
# Create feature branch
git checkout -b feature/new-feature-name

# Keep branch updated with main
git fetch origin
git rebase origin/main

# Clean commit history before PR
git rebase -i origin/main  # Interactive rebase

# Push branch
git push origin feature/new-feature-name
```

### Database Backups

```bash
# Backup PostgreSQL
kubectl --context local-alpha exec -n {product} deploy/postgres -- pg_dump -U postgres {product} > backup.sql

# Restore from backup
kubectl --context local-alpha exec -n {product} deploy/postgres -- psql -U postgres {product} < backup.sql

# Or copy backup to pod and restore
kubectl --context local-alpha cp backup.sql {product}/postgres-pod-name:/tmp/backup.sql
kubectl --context local-alpha exec -n {product} deploy/postgres -- psql -U postgres {product} < /tmp/backup.sql
```

---

## Troubleshooting

### Services Won't Start/Deploy

**Check cluster status**:
```bash
# Verify cluster is running
kubectl cluster-info
kubectl get nodes

# Check namespace exists
kubectl get ns | grep {product}

# Check pod status
kubectl --context local-alpha get pods -n {product}
kubectl --context local-alpha describe pod -n {product} -l app=flask-backend

# View pod events
kubectl --context local-alpha describe pod -n {product} <pod-name>
```

**Check deployment status**:
```bash
# View rollout status
kubectl --context local-alpha rollout status -n {product} deployment/flask-backend

# Check events for deployment
kubectl --context local-alpha describe deployment -n {product} flask-backend

# View logs for debugging
kubectl --context local-alpha logs -n {product} -l app=flask-backend --tail=100
```

**Restart Kubernetes cluster**:
```bash
# MicroK8s
microk8s stop
microk8s start

# Docker Desktop: Restart from menu
# Kind: kind delete cluster && kind create cluster
```

### Kubernetes Context Error

**Verify context configuration**:
```bash
# List available contexts
kubectl config get-contexts

# Set current context
kubectl config use-context local-alpha

# Check current context
kubectl config current-context
```

**Create local-alpha context if missing**:
```bash
# For MicroK8s
microk8s config | kubectl config set-cluster microk8s-cluster --server=$(grep server: ~/.kube/config | awk '{print $2}') --insecure-skip-tls-verify

# For Docker Desktop
kubectl config set-context local-alpha --cluster=docker-desktop --user=docker-desktop
```

### Database Connection Error

```bash
# Verify database pod is running
kubectl --context local-alpha get pod -n {product} -l app=postgres

# Check database pod logs
kubectl --context local-alpha logs -n {product} -l app=postgres

# Try connecting to database
kubectl --context local-alpha exec -n {product} deploy/postgres -- psql -U postgres -c "\l"

# Check if database volume is mounted
kubectl --context local-alpha describe pod -n {product} -l app=postgres | grep -A 5 Volumes
```

### Service Can't Reach Database

```bash
# Verify service DNS works within cluster
kubectl --context local-alpha exec -n {product} deploy/flask-backend -- nslookup postgres

# Check if postgres service exists
kubectl --context local-alpha get svc -n {product} postgres

# Verify network policies (if any)
kubectl --context local-alpha get networkpolicies -n {product}

# Check service endpoints
kubectl --context local-alpha get endpoints -n {product} postgres
```

### Flask Backend Won't Start

```bash
# Check logs for errors
kubectl --context local-alpha logs -n {product} -l app=flask-backend --tail=200

# Check readiness/liveness probe status
kubectl --context local-alpha describe pod -n {product} -l app=flask-backend

# Manually verify database connection
kubectl --context local-alpha exec -n {product} deploy/flask-backend -- python -c "from app import db; print('DB OK')"

# Rebuild the image and restart
docker build -t {image}-flask-backend:latest ./services/flask-backend
kubectl --context local-alpha rollout restart -n {product} deployment/flask-backend
```

### Smoke Tests Failing

**Check which test failed**:
```bash
# Run individually
./tests/smoke/build/test-flask-build.sh
./tests/smoke/api/test-flask-health.sh
./tests/smoke/webui/test-pages-load.sh
```

**Common issues**:
- Pod not ready or crashing (logs: `kubectl --context local-alpha logs -n {product} -l app=<service>`)
- Port-forward not established (check `kubectl --context local-alpha port-forward` process is running)
- API endpoint not implemented
- Missing environment variables (check ConfigMap: `kubectl --context local-alpha get cm -n {product}`)
- Database not initialized (check database logs)

See [Testing Documentation - Smoke Tests](TESTING.md#smoke-tests) for detailed troubleshooting.

### Git Merge Conflicts

```bash
# View conflicts
git status

# Edit conflicted files (marked with <<<<, ====, >>>>)
# Remove conflict markers and keep desired code

# Mark as resolved
git add <resolved-file>

# Complete merge
git commit -m "Resolve merge conflicts"
```

### Slow Image Builds

```bash
# Check Docker disk usage
docker system df

# Clean up unused images/containers
docker system prune

# Rebuild image without cache (slow, but fresh)
docker build --no-cache -t {image}-flask-backend:latest ./services/flask-backend

# Check image layers and sizes
docker history {image}-flask-backend:latest

# Build with buildx for faster multi-arch builds
docker buildx build --platform linux/amd64 -t {image}-flask-backend:latest ./services/flask-backend
```

### QEMU Cross-Architecture Build Issues

**QEMU not available**:
```bash
# Install QEMU support
docker run --rm --privileged multiarch/qemu-user-static --reset -p yes

# Verify buildx setup
docker buildx ls
```

**Slow arm64 build with QEMU**:
```bash
# Expected: 2-5x slower with QEMU emulation
# Use only for final validation, not every iteration

# Build native architecture (fast)
docker buildx build --load .

# Build alternate with QEMU (slow)
docker buildx build --platform linux/arm64 .
```

See [Testing Documentation - Cross-Architecture Testing](TESTING.md#cross-architecture-testing) for complete details.

---

## Tips & Best Practices

### Hot Reload Development

For fastest iteration:
```bash
# Deploy services once
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha

# Edit Python files → auto-reload (FLASK_DEBUG=1)
# Edit JavaScript files → hot reload (Webpack)
# Edit Go files → rebuild image and restart
#   docker build -t {image}-go-backend:latest ./services/go-backend
#   kubectl --context local-alpha rollout restart -n {product} deployment/go-backend
```

### Environment-Specific Configuration

```bash
# Development settings (in Kustomize overlays)
k8s/kustomize/overlays/alpha/kustomization.yaml  # Dev config
k8s/kustomize/overlays/alpha/config.properties   # Dev values

# Production settings (via Kubernetes secrets)
kubectl --context local-alpha create secret generic {product}-secrets -n {product} --from-literal=key=value
kubectl --context prod-cluster create secret generic {product}-secrets -n {product} --from-file=secrets.yaml

# Or use external secret management
Kubernetes Sealed Secrets
AWS Secrets Manager
HashiCorp Vault
```

### Code Organization

Keep project clean:
```bash
# Remove old branches
git branch -D old-branch

# Clean local Docker images
docker image prune -a

# Clean unused K8s resources
kubectl --context local-alpha delete ns {product}  # Delete entire namespace

# Check resource usage
kubectl --context local-alpha top pods -n {product}
```

### Performance Tips

```bash
# Deploy only specific services to reduce memory usage
kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha -l app=postgres,app=flask-backend

# Use lightweight testing
make smoke-test  # Instead of full test suite while developing

# Optimize image layer caching by building in order of change frequency
Dockerfile: base → dependencies → code → entrypoint

# Monitor cluster resources
kubectl --context local-alpha top nodes
kubectl --context local-alpha top pods -n {product}

# Enable resource requests/limits in Kustomize (already configured for alpha)
# Alpha uses: 1 replica, 100m CPU, 128Mi memory
```

---

## Related Documentation

- **Testing**: [Testing Documentation](TESTING.md)
  - Mock data scripts
  - Smoke tests
  - Unit/integration/E2E tests
  - Performance tests
  - Cross-architecture testing

- **Pre-Commit**: [Pre-Commit Checklist](PRE_COMMIT.md)
  - Linting requirements
  - Security scanning
  - Build verification
  - Test requirements

- **Deployment**: [Deployment Guide](deployment/)
  - Image building & optimization
  - Kubernetes deployment (Kustomize & Helm)
  - Health checks
  - Resource management

- **Standards**: [Development Standards](STANDARDS.md)
  - Architecture decisions
  - Code style
  - API conventions
  - Database patterns

- **Workflows**: [CI/CD Workflows](WORKFLOWS.md)
  - GitHub Actions pipelines
  - Build automation
  - Test automation
  - Release processes

---

**Last Updated**: 2026-02-13
**Maintained by**: Penguin Tech Inc

**Note**: Docker Compose is DEPRECATED for local development. All local development uses Kubernetes with Kustomize overlays. Build images with `docker build` and deploy with `kubectl apply` and Kustomize.

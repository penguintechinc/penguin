# Troubleshooting & Debug Guide

This guide provides practical troubleshooting steps for common issues in the project-template environment.

## Table of Contents

1. [Common Issues](#common-issues)
2. [Debug Commands](#debug-commands)
3. [Environment Troubleshooting](#environment-troubleshooting)
4. [Network Troubleshooting](#network-troubleshooting)
5. [Performance Troubleshooting](#performance-troubleshooting)
6. [Log Analysis](#log-analysis)
7. [Support Resources](#support-resources)

---

## Common Issues

### 1. Port Conflicts

**Symptoms**: Services fail to start, "port already in use" error messages

**Quick Diagnosis**:
```bash
lsof -i :5000          # Flask backend
lsof -i :3000          # WebUI frontend
lsof -i :8080          # Go backend
lsof -i :5432          # PostgreSQL
netstat -tlnp
```

**Solutions**:
- Kill existing process: `kill -9 <PID>`
- Use port-forwarding: `kubectl --context local-alpha port-forward -n product svc/flask-backend 5000:5000`
- Check pod status: `kubectl --context local-alpha get pods -n product`

### 2. Database Connection Issues

**Symptoms**: "Connection refused", "password authentication failed", connection timeouts

**Quick Diagnosis**:
```bash
kubectl --context local-alpha exec -n product -it deploy/postgres -- psql -U postgres -d template1 -c "SELECT 1"
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- env | grep DB_
```

**Common Causes & Solutions**:

| Issue | Cause | Solution |
|-------|-------|----------|
| "Connection refused" | Database not running | Run `kubectl apply --context local-alpha -k k8s/kustomize/overlays/alpha` |
| "password authentication failed" | Wrong credentials | Verify DB_USER, DB_PASS in `.env` |
| Connection timeout | Wrong host/port | Check DB_HOST and DB_PORT |
| Galera WSREP_NOT_READY | Galera node not ready | Wait 30 seconds, check logs |

**Verification Steps**:
```bash
kubectl --context local-alpha get pods -n product -l app=postgres
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- env | grep "^DB_"
kubectl --context local-alpha logs -n product -l app=postgres --tail=50
```

### 3. License Validation Failures

**Symptoms**: "License validation failed", feature access denied

**Quick Diagnosis**:
```bash
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- env | grep -i license
curl -v https://license.penguintech.io/api/v2/validate
```

**Common Causes & Solutions**:

| Issue | Cause | Solution |
|-------|-------|----------|
| "Invalid license format" | Malformed key | Verify format: `PENG-XXXX-XXXX-XXXX-XXXX-ABCD` |
| "License expired" | License date passed | Renew through PenguinTech portal |
| "License server unreachable" | Network issue | Check internet, verify LICENSE_SERVER_URL |
| "Development mode" | RELEASE_MODE not set | License checks only in production |

**Verification Steps**:
```bash
make license-validate
make license-check-features
make license-debug
```

### 4. Build Failures

**Symptoms**: Docker build errors, dependency failures, compilation errors

**Quick Diagnosis**:
```bash
docker --version
docker buildx version
docker build -t flask-backend:latest ./services/flask-backend
```

**Python/Flask**:
```bash
docker build -t flask-backend:latest ./services/flask-backend
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- pip list
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- pip cache purge
make clean && make build
```

**Node.js/WebUI**:
```bash
docker build -t webui:latest ./services/webui
kubectl --context local-alpha exec -n product -it deploy/webui -- node --version
kubectl --context local-alpha exec -n product -it deploy/webui -- npm cache clean --force
make clean && make build
```

**Go**:
```bash
docker build -t go-backend:latest ./services/go-backend
kubectl --context local-alpha exec -n product -it deploy/go-backend -- go version
kubectl --context local-alpha exec -n product -it deploy/go-backend -- go mod verify
kubectl --context local-alpha exec -n product -it deploy/go-backend -- go build -v ./...
```

### 5. Test Failures

**Symptoms**: Tests fail locally, pass in CI/CD, inconsistent results

**Quick Diagnosis**:
```bash
make test-unit -- -v
make test-integration -- -v
kubectl --context local-alpha logs -n product --all-containers=true --tail=100 | grep -i test
```

**Common Causes & Solutions**:

| Issue | Cause | Solution |
|-------|-------|----------|
| Pass locally, fail in CI | Environment differences | Check CI environment in `.github/workflows/` |
| Database tests fail | Test DB not initialized | Run `make setup` first |
| Flaky tests | Timing issues | Add retry logic, increase timeouts |
| Port binding failures | Port in use | Use dynamic ports in tests |

---

## Debug Commands

### Container Debugging

```bash
kubectl --context local-alpha logs -n product -l app=flask-backend
kubectl --context local-alpha logs -n product -l app=go-backend -f          # Follow logs
kubectl --context local-alpha logs -n product -l app=webui --tail=100       # Last 100 lines
kubectl --context local-alpha logs -n product -l app=flask-backend --timestamps=true

kubectl --context local-alpha logs -n product -l app=flask-backend | grep -i error
kubectl --context local-alpha logs -n product -l app=flask-backend | grep -i warning

kubectl --context local-alpha exec -n product -it deploy/flask-backend -- /bin/bash
kubectl --context local-alpha exec -n product -it deploy/postgres -- psql -U postgres

kubectl --context local-alpha exec -n product -it deploy/flask-backend -- ls -la /app
```

### Application Debugging

```bash
make debug                                  # Start with debug flags
make logs                                   # View application logs
make health                                 # Check service health

curl http://localhost:5000/healthz          # Flask
curl http://localhost:3000/health           # WebUI
curl http://localhost:8080/healthz          # Go
```

### License Debugging

```bash
make license-debug
make license-validate
make license-check-features
kubectl --context local-alpha logs -n product -l app=flask-backend | grep -i license
```

---

## Environment Troubleshooting

### Configuration Issues

```bash
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- env | sort
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- env | grep DB_
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- env | grep SECRET_
ls -la .env
```

**Load order**:
1. `.env` file loaded first
2. Kubernetes ConfigMap overrides `.env`
3. Container environment takes precedence

### Python Environment Issues

```bash
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- python3 --version
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- which python3
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- pip list
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- pip check
```

### Node.js Environment Issues

```bash
kubectl --context local-alpha exec -n product -it deploy/webui -- node --version
kubectl --context local-alpha exec -n product -it deploy/webui -- npm --version
kubectl --context local-alpha exec -n product -it deploy/webui -- ls node_modules | head -20
kubectl --context local-alpha exec -n product -it deploy/webui -- ls -la build/
```

---

## Network Troubleshooting

### Container Communication

```bash
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- curl http://webui:3000
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- curl http://go-backend:8080
kubectl --context local-alpha exec -n product -it deploy/webui -- curl http://flask-backend:5000/api/v1/health

kubectl --context local-alpha exec -n product -it deploy/flask-backend -- ping webui
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- getent hosts postgres

kubectl --context local-alpha get pods -n product -o wide
kubectl --context local-alpha describe pod -n product -l app=flask-backend
```

### DNS Resolution

```bash
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- nslookup postgres
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- getent hosts webui
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- cat /etc/hosts
```

### Port Binding Issues

```bash
kubectl --context local-alpha port-forward -n product svc/flask-backend 5000:5000
kubectl --context local-alpha port-forward -n product svc/webui 3000:3000
netstat -tlnp | grep LISTEN
ss -tlnp | grep LISTEN
```

---

## Performance Troubleshooting

### High CPU Usage

```bash
kubectl --context local-alpha top pod -n product
kubectl --context local-alpha top pod -n product -l app=flask-backend
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- python3 -m cProfile app.py
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- top -n 1
```

### Memory Issues

```bash
kubectl --context local-alpha top pod -n product
kubectl --context local-alpha describe pod -n product -l app=flask-backend | grep -A5 "Requests\|Limits"
kubectl --context local-alpha exec -n product -it deploy/flask-backend -- ps aux --sort=-%mem
```

### Slow Queries

```bash
kubectl --context local-alpha exec -n product -it deploy/postgres -- psql -U postgres -c \
  "ALTER SYSTEM SET log_min_duration_statement = 1000;"
kubectl --context local-alpha logs -n product -l app=postgres | grep slow
kubectl --context local-alpha exec -n product -it deploy/postgres -- psql -U postgres -d your_db \
  -c "EXPLAIN ANALYZE SELECT * FROM users LIMIT 10;"
```

### Bottleneck Analysis

```bash
kubectl --context local-alpha top pod -n product
curl -w "@curl-format.txt" -o /dev/null -s http://localhost:5000/api/v1/users
```

---

## Log Analysis

### Finding Errors and Warnings

```bash
kubectl --context local-alpha logs -n product --all-containers=true | grep -i error
kubectl --context local-alpha logs -n product --all-containers=true | grep -B2 -A2 "error"
kubectl --context local-alpha logs -n product -l app=flask-backend | grep ERROR
kubectl --context local-alpha logs -n product -l app=go-backend | grep WARN
kubectl --context local-alpha logs -n product --all-containers=true | grep -i error | wc -l
```

### Analyzing Specific Issues

```bash
# Authentication errors
kubectl --context local-alpha logs -n product --all-containers=true | grep -i "auth\|permission\|unauthorized"

# Database errors
kubectl --context local-alpha logs -n product --all-containers=true | grep -i "connection\|query\|database"

# License errors
kubectl --context local-alpha logs -n product --all-containers=true | grep -i "license\|validation"

# Network errors
kubectl --context local-alpha logs -n product --all-containers=true | grep -i "timeout\|refused\|unreachable"
```

### Saving Logs for Analysis

```bash
kubectl --context local-alpha logs -n product --all-containers=true > /tmp/project-logs.txt
kubectl --context local-alpha logs -n product --all-containers=true --timestamps=true > /tmp/project-logs-ts.txt
```

---

## Support Resources

### Documentation

- **Technical Documentation**: [Development Standards](../STANDARDS.md)
- **License Integration**: [License Server Guide](../licensing/license-server-integration.md)
- **Kubernetes Deployment**: [Kubernetes Guide](../KUBERNETES.md)
- **Workflow Documentation**: [CI/CD Workflows](../WORKFLOWS.md)

### Getting Help

- **Technical Support**: support@penguintech.io
- **Sales Inquiries**: sales@penguintech.io
- **License Issues**: licenses@penguintech.io

### System Status

- **License Server Status**: https://status.penguintech.io
- **PenguinTech Status Page**: https://www.penguintech.io/status

---

## Quick Reference Checklist

When troubleshooting, verify:

- [ ] Service is running: `kubectl --context local-alpha get pods -n product`
- [ ] Correct ports mapped: `kubectl --context local-alpha port-forward -n product svc/<service> <port>:<port>`
- [ ] Environment variables set: `kubectl --context local-alpha exec -n product -it deploy/<service> -- env`
- [ ] Database accessible: Test connection from app container
- [ ] Network connectivity: Ping between containers
- [ ] Logs show no errors: `kubectl --context local-alpha logs -n product -l app=<service> --tail=50`
- [ ] Recent changes reviewed: `git diff`
- [ ] Clean rebuild attempted: `make clean && make build && kubectl --context local-alpha rollout restart deployment/<service> -n product`
- [ ] All containers healthy: `make health`

---

**Last Updated**: December 2025
**Version**: 1.0.0

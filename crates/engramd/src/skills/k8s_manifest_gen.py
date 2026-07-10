#!/usr/bin/env python3
"""k8s_manifest_gen — Engram skill (no network). Generate K8s Deployment+Service.

Builds ready-to-apply Kubernetes YAML (by hand as a string — no PyYAML) for a
Deployment (replicas, container image/port, CPU/memory requests+limits, env
vars, and readiness/liveness probe stubs on the port) plus a Service exposing
it. Stdlib only, no network. Validates name + image.

Request (stdin): {"name": "web", "image": "nginx:1.27", "port": 8080,
                  "replicas": 3, "env": {"LOG_LEVEL": "info"},
                  "cpu": "100m", "memory": "128Mi", "service_type": "LoadBalancer"}
Output (stdout): {name, resources: ["Deployment","Service"], manifest}
"""
import json
import re
import sys

VALID_SERVICE_TYPES = {"ClusterIP", "NodePort", "LoadBalancer"}
NAME_RE = re.compile(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$")  # RFC 1123 label


def _yaml_scalar(v):
    # Quote strings that could be misread as non-strings or contain special chars.
    s = str(v)
    if s == "":
        return '""'
    if re.fullmatch(r"[A-Za-z0-9_./:+@-]+", s) and not re.fullmatch(r"(true|false|null|yes|no|on|off|~)", s, re.IGNORECASE) and not re.fullmatch(r"-?\d+(\.\d+)?", s):
        return s
    return '"%s"' % s.replace("\\", "\\\\").replace('"', '\\"')


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    example = {"name": "web", "image": "nginx:1.27", "port": 8080, "replicas": 2}
    name = q.get("name")
    image = q.get("image")
    if not name or not isinstance(name, str) or not name.strip():
        print(json.dumps({"error": "missing required field: name", "example": example}))
        return 0
    if not image or not isinstance(image, str) or not image.strip():
        print(json.dumps({"error": "missing required field: image", "example": example}))
        return 0
    name = name.strip()
    image = image.strip()
    if not NAME_RE.match(name) or len(name) > 63:
        print(json.dumps({
            "error": "name must be a valid RFC1123 label (lowercase alphanumerics and '-', <=63 chars)",
            "example": example,
        }))
        return 0

    try:
        port = int(q.get("port", 80) or 80)
        replicas = int(q.get("replicas", 2) or 2)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'port' and 'replicas' must be integers", "example": example}))
        return 0
    if not (1 <= port <= 65535):
        print(json.dumps({"error": "'port' must be between 1 and 65535"}))
        return 0
    if replicas < 1:
        print(json.dumps({"error": "'replicas' must be >= 1"}))
        return 0

    cpu = str(q.get("cpu", "100m") or "100m")
    memory = str(q.get("memory", "128Mi") or "128Mi")
    service_type = str(q.get("service_type", "ClusterIP") or "ClusterIP")
    if service_type not in VALID_SERVICE_TYPES:
        print(json.dumps({
            "error": "service_type must be one of %s" % sorted(VALID_SERVICE_TYPES),
            "example": example,
        }))
        return 0

    env = q.get("env") or {}
    if not isinstance(env, dict):
        print(json.dumps({"error": "'env' must be an object of {NAME: value}"}))
        return 0

    L = []  # lines
    # ---- Deployment ----
    L.append("apiVersion: apps/v1")
    L.append("kind: Deployment")
    L.append("metadata:")
    L.append("  name: %s" % name)
    L.append("  labels:")
    L.append("    app: %s" % name)
    L.append("spec:")
    L.append("  replicas: %d" % replicas)
    L.append("  selector:")
    L.append("    matchLabels:")
    L.append("      app: %s" % name)
    L.append("  template:")
    L.append("    metadata:")
    L.append("      labels:")
    L.append("        app: %s" % name)
    L.append("    spec:")
    L.append("      containers:")
    L.append("        - name: %s" % name)
    L.append("          image: %s" % _yaml_scalar(image))
    L.append("          ports:")
    L.append("            - containerPort: %d" % port)
    if env:
        L.append("          env:")
        for k in env:
            L.append("            - name: %s" % _yaml_scalar(str(k)))
            L.append("              value: %s" % _yaml_scalar(env[k]))
    L.append("          resources:")
    L.append("            requests:")
    L.append("              cpu: %s" % _yaml_scalar(cpu))
    L.append("              memory: %s" % _yaml_scalar(memory))
    L.append("            limits:")
    L.append("              cpu: %s" % _yaml_scalar(cpu))
    L.append("              memory: %s" % _yaml_scalar(memory))
    L.append("          readinessProbe:")
    L.append("            tcpSocket:")
    L.append("              port: %d" % port)
    L.append("            initialDelaySeconds: 5")
    L.append("            periodSeconds: 10")
    L.append("          livenessProbe:")
    L.append("            tcpSocket:")
    L.append("              port: %d" % port)
    L.append("            initialDelaySeconds: 15")
    L.append("            periodSeconds: 20")

    L.append("---")
    # ---- Service ----
    L.append("apiVersion: v1")
    L.append("kind: Service")
    L.append("metadata:")
    L.append("  name: %s" % name)
    L.append("  labels:")
    L.append("    app: %s" % name)
    L.append("spec:")
    L.append("  type: %s" % service_type)
    L.append("  selector:")
    L.append("    app: %s" % name)
    L.append("  ports:")
    L.append("    - protocol: TCP")
    L.append("      port: %d" % port)
    L.append("      targetPort: %d" % port)

    manifest = "\n".join(L) + "\n"
    result = {
        "name": name,
        "resources": ["Deployment", "Service"],
        "service_type": service_type,
        "replicas": replicas,
        "port": port,
        "manifest": manifest,
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())

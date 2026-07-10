#!/usr/bin/env python3
"""cloud_service_picker — Engram skill (no network). AWS/GCP/Azure equivalents.

Maps a plain-English infrastructure need (e.g. "object storage", "serverless
functions", "managed sql", "kubernetes") to the equivalent managed service on
AWS, GCP, and Azure, with a short note. Fuzzy, case-insensitive matching via
substring + difflib. Omit 'need' to dump the whole table; unmatched needs list
all supported categories. Stdlib only (difflib) — no network.

Request (stdin): {"need": "object storage"}
Output (stdout): {need, matched, aws, gcp, azure, notes}
             or  {supported: [...], table: {...}}   (when no/unknown need)
"""
import difflib
import json
import sys

# need -> (aliases, aws, gcp, azure, notes)
TABLE = {
    "object storage": (["blob storage", "file storage", "s3", "bucket", "buckets"],
                       "S3", "Cloud Storage", "Blob Storage",
                       "Durable, cheap storage for files/blobs; not a filesystem."),
    "managed sql": (["relational database", "rdbms", "postgres", "mysql", "sql database", "rds"],
                    "RDS", "Cloud SQL", "Azure SQL Database",
                    "Managed relational DB (Postgres/MySQL/SQL Server) with backups & failover."),
    "serverless functions": (["serverless", "functions", "faas", "lambda", "function as a service"],
                             "Lambda", "Cloud Functions", "Azure Functions",
                             "Event-driven, pay-per-invocation compute; no servers to manage."),
    "message queue": (["queue", "messaging", "pubsub", "pub/sub", "pub-sub", "sqs", "event bus"],
                      "SQS", "Pub/Sub", "Service Bus",
                      "Decouple producers/consumers; async work distribution."),
    "kubernetes": (["managed kubernetes", "k8s", "container orchestration", "eks", "gke", "aks"],
                   "EKS", "GKE", "AKS",
                   "Managed Kubernetes control plane for orchestrating containers."),
    "cdn": (["content delivery network", "edge cache", "cloudfront"],
            "CloudFront", "Cloud CDN", "Azure CDN",
            "Edge caching to serve static assets close to users."),
    "nosql": (["nosql database", "document database", "key value", "key-value store", "dynamodb", "firestore"],
              "DynamoDB", "Firestore", "Cosmos DB",
              "Horizontally-scalable non-relational store (document/key-value)."),
    "cache": (["in-memory cache", "redis", "memcached", "caching", "elasticache"],
              "ElastiCache", "Memorystore", "Azure Cache for Redis",
              "Managed Redis/Memcached for low-latency in-memory caching."),
    "load balancer": (["load balancing", "alb", "elb", "reverse proxy", "traffic distribution"],
                      "Elastic Load Balancing (ALB/NLB)", "Cloud Load Balancing", "Azure Load Balancer",
                      "Distribute incoming traffic across healthy instances."),
    "secrets": (["secret management", "secret manager", "key vault", "credentials", "secrets manager", "vault"],
                "Secrets Manager", "Secret Manager", "Key Vault",
                "Store and rotate API keys, passwords, and certificates securely."),
    "data warehouse": (["warehouse", "analytics database", "olap", "redshift", "bigquery", "synapse"],
                       "Redshift", "BigQuery", "Synapse Analytics",
                       "Columnar store for large-scale analytical (OLAP) queries."),
    "container registry": (["docker registry", "image registry", "ecr", "gcr", "acr"],
                           "ECR", "Artifact Registry", "Azure Container Registry",
                           "Private registry for Docker/OCI container images."),
    "dns": (["domain name system", "managed dns", "route 53", "route53"],
            "Route 53", "Cloud DNS", "Azure DNS",
            "Managed authoritative DNS and health-based routing."),
    "monitoring": (["observability", "metrics", "logging", "cloudwatch", "apm"],
                   "CloudWatch", "Cloud Monitoring (Operations Suite)", "Azure Monitor",
                   "Metrics, logs, dashboards, and alerting for your workloads."),
    "virtual machines": (["vm", "compute instance", "servers", "ec2", "instances"],
                         "EC2", "Compute Engine", "Azure Virtual Machines",
                         "Resizable on-demand virtual servers (IaaS compute)."),
}


def _match(need):
    n = need.strip().lower()
    if not n:
        return None
    # exact key
    if n in TABLE:
        return n
    # substring either direction against key or alias
    for key, (aliases, *_rest) in TABLE.items():
        candidates = [key] + list(aliases)
        for c in candidates:
            if n == c or n in c or c in n:
                return key
    # fuzzy against all keys + aliases
    pool = {}
    for key, (aliases, *_rest) in TABLE.items():
        pool[key] = key
        for a in aliases:
            pool[a] = key
    close = difflib.get_close_matches(n, list(pool.keys()), n=1, cutoff=0.6)
    if close:
        return pool[close[0]]
    return None


def _row(key):
    aliases, aws, gcp, azure, notes = TABLE[key]
    return {"aws": aws, "gcp": gcp, "azure": azure, "notes": notes}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    need = q.get("need")
    if need is None or (isinstance(need, str) and not need.strip()):
        # dump full table
        table = {k: _row(k) for k in sorted(TABLE)}
        print(json.dumps({
            "supported": sorted(TABLE),
            "table": table,
        }, indent=2, default=str))
        return 0

    if not isinstance(need, str):
        print(json.dumps({"error": "'need' must be a string", "example": {"need": "object storage"}}))
        return 0

    key = _match(need)
    if key is None:
        print(json.dumps({
            "need": need,
            "matched": None,
            "message": "no close match; here are the supported needs",
            "supported": sorted(TABLE),
        }, indent=2, default=str))
        return 0

    row = _row(key)
    result = {"need": need, "matched": key}
    result.update(row)
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())

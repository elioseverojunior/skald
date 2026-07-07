// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Benchmark fixture data.
//!
//! Provides YAML and JSON constants at three sizes (small, medium, large)
//! for throughput benchmarking across the Skald pipeline.

use std::fmt::Write;

// ─── Small: K8s Pod Spec (~200 bytes) ────────────────────────────────

/// Minimal Kubernetes pod spec exercising plain scalars, mappings, and sequences.
pub const SMALL_POD: &str = "\
apiVersion: v1
kind: Pod
metadata:
  name: nginx
  labels:
    app: web
    tier: frontend
spec:
  containers:
    - name: nginx
      image: nginx:1.27
      ports:
        - containerPort: 80
          protocol: TCP
";

/// JSON equivalent of [`SMALL_POD`].
pub const SMALL_POD_JSON: &str = r#"{
  "apiVersion": "v1",
  "kind": "Pod",
  "metadata": {
    "name": "nginx",
    "labels": {
      "app": "web",
      "tier": "frontend"
    }
  },
  "spec": {
    "containers": [
      {
        "name": "nginx",
        "image": "nginx:1.27",
        "ports": [
          {
            "containerPort": 80,
            "protocol": "TCP"
          }
        ]
      }
    ]
  }
}"#;

// ─── Medium: Helm Values (~5KB) ─────────────────────────────────────

/// Realistic Helm values.yaml exercising nested maps, flow sequences,
/// block scalars, quoted strings, comments, and anchors/aliases.
pub const MEDIUM_HELM: &str = "\
# Application configuration
replicaCount: 3

image:
  repository: registry.example.com/app
  tag: \"2.4.1\"
  pullPolicy: IfNotPresent

nameOverride: \"\"
fullnameOverride: my-application

serviceAccount:
  create: true
  annotations:
    eks.amazonaws.com/role-arn: \"arn:aws:iam::123456789012:role/app-role\"
  name: app-sa

podAnnotations:
  prometheus.io/scrape: \"true\"
  prometheus.io/port: \"9090\"

podSecurityContext:
  runAsNonRoot: true
  runAsUser: 1000
  fsGroup: 2000

securityContext:
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop: [ALL]

service:
  type: ClusterIP
  port: 80
  targetPort: 8080
  annotations: {}

ingress:
  enabled: true
  className: nginx
  annotations:
    nginx.ingress.kubernetes.io/rewrite-target: /
    cert-manager.io/cluster-issuer: letsencrypt-prod
  hosts:
    - host: app.example.com
      paths:
        - path: /
          pathType: Prefix
        - path: /api
          pathType: Prefix
    - host: api.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: app-tls
      hosts: [app.example.com, api.example.com]

resources: &default_resources
  limits:
    cpu: 500m
    memory: 512Mi
  requests:
    cpu: 100m
    memory: 128Mi

autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 75
  targetMemoryUtilizationPercentage: 80

nodeSelector:
  kubernetes.io/os: linux
  node.kubernetes.io/instance-type: m5.large

tolerations:
  - key: dedicated
    operator: Equal
    value: app
    effect: NoSchedule

affinity:
  podAntiAffinity:
    preferredDuringSchedulingIgnoredDuringExecution:
      - weight: 100
        podAffinityTerm:
          labelSelector:
            matchExpressions:
              - key: app
                operator: In
                values: [web, api]
          topologyKey: kubernetes.io/hostname

env:
  - name: DATABASE_URL
    valueFrom:
      secretKeyRef:
        name: db-credentials
        key: url
  - name: LOG_LEVEL
    value: info
  - name: CACHE_TTL
    value: \"3600\"
  - name: FEATURES
    value: \"auth,metrics,tracing\"

configMap:
  data:
    application.yaml: |
      server:
        port: 8080
        shutdown-timeout: 30s
      logging:
        level: INFO
        format: json
      database:
        pool:
          min-idle: 5
          max-size: 20
          timeout: 10s
      cache:
        type: redis
        ttl: 3600
    nginx.conf: |
      upstream backend {
        server 127.0.0.1:8080;
        keepalive 32;
      }
      server {
        listen 80;
        location / {
          proxy_pass http://backend;
          proxy_set_header Host $host;
          proxy_set_header X-Real-IP $remote_addr;
        }
      }

sidecars:
  - name: log-collector
    image: fluent/fluent-bit:2.2
    resources: *default_resources
    volumeMounts:
      - name: logs
        mountPath: /var/log/app

volumes:
  - name: logs
    emptyDir: {}
  - name: config
    configMap:
      name: app-config

livenessProbe:
  httpGet:
    path: /healthz
    port: 8080
  initialDelaySeconds: 15
  periodSeconds: 10
  failureThreshold: 3

readinessProbe:
  httpGet:
    path: /readyz
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 5

metrics:
  enabled: true
  serviceMonitor:
    enabled: true
    interval: 30s
    labels:
      release: prometheus
";

/// JSON equivalent of [`MEDIUM_HELM`].
pub const MEDIUM_HELM_JSON: &str = r#"{
  "replicaCount": 3,
  "image": {
    "repository": "registry.example.com/app",
    "tag": "2.4.1",
    "pullPolicy": "IfNotPresent"
  },
  "nameOverride": "",
  "fullnameOverride": "my-application",
  "serviceAccount": {
    "create": true,
    "annotations": {
      "eks.amazonaws.com/role-arn": "arn:aws:iam::123456789012:role/app-role"
    },
    "name": "app-sa"
  },
  "podAnnotations": {
    "prometheus.io/scrape": "true",
    "prometheus.io/port": "9090"
  },
  "podSecurityContext": {
    "runAsNonRoot": true,
    "runAsUser": 1000,
    "fsGroup": 2000
  },
  "securityContext": {
    "allowPrivilegeEscalation": false,
    "readOnlyRootFilesystem": true,
    "capabilities": {
      "drop": ["ALL"]
    }
  },
  "service": {
    "type": "ClusterIP",
    "port": 80,
    "targetPort": 8080,
    "annotations": {}
  },
  "ingress": {
    "enabled": true,
    "className": "nginx",
    "annotations": {
      "nginx.ingress.kubernetes.io/rewrite-target": "/",
      "cert-manager.io/cluster-issuer": "letsencrypt-prod"
    },
    "hosts": [
      {
        "host": "app.example.com",
        "paths": [
          {"path": "/", "pathType": "Prefix"},
          {"path": "/api", "pathType": "Prefix"}
        ]
      },
      {
        "host": "api.example.com",
        "paths": [
          {"path": "/", "pathType": "Prefix"}
        ]
      }
    ],
    "tls": [
      {
        "secretName": "app-tls",
        "hosts": ["app.example.com", "api.example.com"]
      }
    ]
  },
  "resources": {
    "limits": {"cpu": "500m", "memory": "512Mi"},
    "requests": {"cpu": "100m", "memory": "128Mi"}
  },
  "autoscaling": {
    "enabled": true,
    "minReplicas": 2,
    "maxReplicas": 10,
    "targetCPUUtilizationPercentage": 75,
    "targetMemoryUtilizationPercentage": 80
  },
  "nodeSelector": {
    "kubernetes.io/os": "linux",
    "node.kubernetes.io/instance-type": "m5.large"
  },
  "tolerations": [
    {
      "key": "dedicated",
      "operator": "Equal",
      "value": "app",
      "effect": "NoSchedule"
    }
  ],
  "affinity": {
    "podAntiAffinity": {
      "preferredDuringSchedulingIgnoredDuringExecution": [
        {
          "weight": 100,
          "podAffinityTerm": {
            "labelSelector": {
              "matchExpressions": [
                {
                  "key": "app",
                  "operator": "In",
                  "values": ["web", "api"]
                }
              ]
            },
            "topologyKey": "kubernetes.io/hostname"
          }
        }
      ]
    }
  },
  "env": [
    {"name": "DATABASE_URL", "valueFrom": {"secretKeyRef": {"name": "db-credentials", "key": "url"}}},
    {"name": "LOG_LEVEL", "value": "info"},
    {"name": "CACHE_TTL", "value": "3600"},
    {"name": "FEATURES", "value": "auth,metrics,tracing"}
  ],
  "configMap": {
    "data": {
      "application.yaml": "server:\n  port: 8080\n  shutdown-timeout: 30s\nlogging:\n  level: INFO\n  format: json\ndatabase:\n  pool:\n    min-idle: 5\n    max-size: 20\n    timeout: 10s\ncache:\n  type: redis\n  ttl: 3600\n",
      "nginx.conf": "upstream backend {\n  server 127.0.0.1:8080;\n  keepalive 32;\n}\nserver {\n  listen 80;\n  location / {\n    proxy_pass http://backend;\n    proxy_set_header Host $host;\n    proxy_set_header X-Real-IP $remote_addr;\n  }\n}\n"
    }
  },
  "sidecars": [
    {
      "name": "log-collector",
      "image": "fluent/fluent-bit:2.2",
      "resources": {
        "limits": {"cpu": "500m", "memory": "512Mi"},
        "requests": {"cpu": "100m", "memory": "128Mi"}
      },
      "volumeMounts": [
        {"name": "logs", "mountPath": "/var/log/app"}
      ]
    }
  ],
  "volumes": [
    {"name": "logs", "emptyDir": {}},
    {"name": "config", "configMap": {"name": "app-config"}}
  ],
  "livenessProbe": {
    "httpGet": {"path": "/healthz", "port": 8080},
    "initialDelaySeconds": 15,
    "periodSeconds": 10,
    "failureThreshold": 3
  },
  "readinessProbe": {
    "httpGet": {"path": "/readyz", "port": 8080},
    "initialDelaySeconds": 5,
    "periodSeconds": 5
  },
  "metrics": {
    "enabled": true,
    "serviceMonitor": {
      "enabled": true,
      "interval": "30s",
      "labels": {
        "release": "prometheus"
      }
    }
  }
}"#;

// ─── Large: Programmatic Generator ──────────────────────────────────

/// Generates a large YAML document with `n` entries.
///
/// Each entry is a mapping with string, integer, float, boolean, sequence,
/// and nested mapping fields — roughly 125 bytes per entry.
/// `generate_large(800)` produces ~100KB.
pub fn generate_large(n: usize) -> String {
    let mut out = String::with_capacity(n * 130);
    writeln!(out, "entries:").unwrap();
    for i in 0..n {
        writeln!(out, "  - id: {i}").unwrap();
        writeln!(out, "    name: \"entry-{i}\"").unwrap();
        writeln!(out, "    score: {:.2}", i as f64 * 0.37).unwrap();
        writeln!(out, "    active: {}", i % 2 == 0).unwrap();
        writeln!(out, "    tags: [alpha, bravo, charlie]").unwrap();
        writeln!(out, "    meta:").unwrap();
        writeln!(out, "      region: us-east-{}", (i % 4) + 1).unwrap();
        writeln!(out, "      weight: {}", i * 10 + 5).unwrap();
    }
    out
}

/// Generates the JSON equivalent of [`generate_large`].
pub fn generate_large_json(n: usize) -> String {
    let mut out = String::with_capacity(n * 180);
    out.push_str("{\"entries\":[");
    for i in 0..n {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            concat!(
                "{{\"id\":{id},\"name\":\"entry-{id}\",\"score\":{score:.2},",
                "\"active\":{active},\"tags\":[\"alpha\",\"bravo\",\"charlie\"],",
                "\"meta\":{{\"region\":\"us-east-{region}\",\"weight\":{weight}}}}}"
            ),
            id = i,
            score = i as f64 * 0.37,
            active = i % 2 == 0,
            region = (i % 4) + 1,
            weight = i * 10 + 5,
        )
        .unwrap();
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_pod_parses_with_skald() {
        skald_ast::composer::compose_all(SMALL_POD).unwrap();
    }

    #[test]
    fn small_pod_parses_with_yaml_rust2() {
        yaml_rust2::YamlLoader::load_from_str(SMALL_POD).unwrap();
    }

    #[test]
    fn medium_helm_parses_with_skald() {
        skald_ast::composer::compose_all(MEDIUM_HELM).unwrap();
    }

    #[test]
    fn medium_helm_parses_with_yaml_rust2() {
        yaml_rust2::YamlLoader::load_from_str(MEDIUM_HELM).unwrap();
    }

    #[test]
    fn large_parses_with_skald() {
        let large = generate_large(100);
        skald_ast::composer::compose_all(&large).unwrap();
    }

    #[test]
    fn large_parses_with_yaml_rust2() {
        let large = generate_large(100);
        yaml_rust2::YamlLoader::load_from_str(&large).unwrap();
    }

    #[test]
    fn json_equivalents_parse_with_serde_json() {
        serde_json::from_str::<serde_json::Value>(SMALL_POD_JSON).unwrap();
        serde_json::from_str::<serde_json::Value>(MEDIUM_HELM_JSON).unwrap();
        let large_json = generate_large_json(100);
        serde_json::from_str::<serde_json::Value>(&large_json).unwrap();
    }

    #[test]
    fn large_generator_produces_expected_size() {
        let large = generate_large(800);
        // ~125 bytes per entry + 9 bytes header = ~100KB
        assert!(large.len() > 80_000, "expected >80KB, got {}", large.len());
        assert!(
            large.len() < 200_000,
            "expected <200KB, got {}",
            large.len()
        );
    }

    #[test]
    fn fixture_sizes_are_in_expected_ranges() {
        assert!(
            SMALL_POD.len() > 100 && SMALL_POD.len() < 500,
            "SMALL_POD: {} bytes",
            SMALL_POD.len()
        );
        assert!(
            MEDIUM_HELM.len() > 2_000 && MEDIUM_HELM.len() < 10_000,
            "MEDIUM_HELM: {} bytes",
            MEDIUM_HELM.len()
        );
    }
}

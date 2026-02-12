# Cuervo CLI - Configuración Avanzada

Este archivo contiene ejemplos completos de configuración para Cuervo CLI, organizados por casos de uso.

## Tabla de Contenidos
1. [Configuración Básica](#configuración-básica)
2. [Configuración de Proveedores](#configuración-de-proveedores)
3. [Configuración de Herramientas](#configuración-de-herramientas)
4. [Configuración de Seguridad](#configuración-de-seguridad)
5. [Configuración de Memoria](#configuración-de-memoria)
6. [Configuración de Rendimiento](#configuración-de-rendimiento)
7. [Configuración para Desarrollo](#configuración-para-desarrollo)
8. [Configuración Enterprise](#configuración-enterprise)

## Configuración Básica

### Configuración Mínima Funcional
```toml
# ~/.cuervo/config.toml
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"
max_tokens = 8192
temperature = 0.0

# Configuración básica de proveedores
[models.providers.anthropic]
enabled = true
api_key_env = "ANTHROPIC_API_KEY"

[models.providers.ollama]
enabled = true
api_base = "http://localhost:11434"
default_model = "llama3.2"
```

### Configuración con Preferencias de Usuario
```toml
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"
max_tokens = 4096
temperature = 0.7
top_p = 0.9
frequency_penalty = 0.0
presence_penalty = 0.0

[display]
theme = "dark"
brand_color = "#8B5CF6"  # Violet
show_banner = true
show_usage = true
stream_output = true

[logging]
level = "info"
format = "pretty"
file = "~/.cuervo/logs/cuervo.log"
```

## Configuración de Proveedores

### Configuración Multi-Proveedor
```toml
[models.providers.anthropic]
enabled = true
api_base = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-5-20250929"
timeout_secs = 30
max_retries = 3
fallback_to = "openai"

[models.providers.openai]
enabled = true
api_base = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-4o"
organization = "org-xxx"  # Opcional para equipos

[models.providers.ollama]
enabled = true
api_base = "http://localhost:11434"
default_model = "llama3.2"
keep_alive = "5m"
timeout_secs = 120  # Más tiempo para modelos locales

[models.providers.gemini]
enabled = true
api_key_env = "GEMINI_API_KEY"
default_model = "gemini-1.5-pro"
safety_settings = "moderate"

[models.providers.deepseek]
enabled = true
api_base = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"
default_model = "deepseek-chat"

[models.providers.openai_compat]
enabled = true
api_base = "http://localhost:8080/v1"  # LM Studio, LocalAI, etc.
api_key_env = "LOCAL_API_KEY"
default_model = "local-model"
```

### Configuración de Fallback y Resiliencia
```toml
[models.fallback]
enabled = true
strategy = "sequential"  # sequential, parallel, smart
providers = ["anthropic", "openai", "ollama"]
health_check_interval = 30
circuit_breaker_threshold = 5
circuit_breaker_timeout = 60

[models.load_balancing]
enabled = true
strategy = "round_robin"  # round_robin, latency_based, cost_based
providers = ["anthropic", "openai"]
weights = [60, 40]  # 60% Anthropic, 40% OpenAI
```

### Configuración de Modelos Específicos
```toml
[models.custom]
# Modelos personalizados por proveedor
anthropic = [
    "claude-3-5-sonnet-20241022",
    "claude-3-opus-20240229",
    "claude-3-haiku-20240307",
]

openai = [
    "gpt-4o",
    "gpt-4-turbo",
    "gpt-3.5-turbo",
]

ollama = [
    "llama3.2",
    "mistral",
    "codellama",
    "phi",
]

# Alias para modelos
[models.aliases]
fast = "claude-3-haiku-20240307"
smart = "claude-3-5-sonnet-20241022"
code = "codellama:latest"
creative = "gpt-4o"
```

## Configuración de Herramientas

### Permisos y Seguridad de Herramientas
```toml
[tools]
# Configuración general
confirm_destructive = true
timeout_secs = 120
max_output_size = 1048576  # 1MB

# Directorios permitidos (lista vacía = todos)
allowed_directories = [
    "/home/user/projects",
    "/home/user/documents",
    "/tmp",
]

# Directorios bloqueados (siempre denegados)
blocked_directories = [
    "/etc",
    "/root",
    "/home/user/.ssh",
    "/home/user/.config",
]

# Patrones de archivos bloqueados
blocked_patterns = [
    "**/.env",
    "**/.env.*",
    "**/credentials.json",
    "**/*.pem",
    "**/*.key",
    "**/*.secret",
    "**/secrets/*",
    "**/passwords*",
]

# Configuración por herramienta
[tools.bash]
enabled = true
permission_level = "destructive"
require_confirmation = true
allowed_commands = ["git", "cargo", "npm", "python", "node"]
blocked_commands = ["rm", "mv", "dd", "format", "shutdown"]

[tools.file_write]
enabled = true
permission_level = "read_write"
require_confirmation = true
max_file_size = 5242880  # 5MB
allowed_extensions = [".rs", ".toml", ".md", ".json", ".yml", ".yaml"]

[tools.file_edit]
enabled = true
permission_level = "read_write"
require_confirmation = true
backup_original = true
max_changes_per_operation = 10

[tools.web_fetch]
enabled = true
permission_level = "read_only"
timeout_secs = 30
max_response_size = 5242880  # 5MB
allowed_domains = ["github.com", "docs.rs", "crates.io"]
blocked_domains = []
```

### Configuración de Sandbox
```toml
[tools.sandbox]
enabled = true
type = "namespace"  # namespace, chroot, container
resource_limits = { memory_mb = 512, cpu_percent = 50 }
network_access = false
readonly_rootfs = true

# Namespace configuration (Linux only)
[tools.sandbox.namespaces]
pid = true
net = true
ipc = true
uts = true
mount = true
user = true

# Seccomp filters
[tools.sandbox.seccomp]
enabled = true
policy = "strict"  # strict, moderate, permissive
```

## Configuración de Seguridad

### Detección y Manejo de PII
```toml
[security]
# Detección de información personal
pii_detection = true
pii_action = "warn"  # warn, block, redact
pii_confidence_threshold = 0.8

# Tipos de PII a detectar
pii_types = [
    "email",
    "phone",
    "credit_card",
    "ssn",
    "passport",
    "ip_address",
    "mac_address",
]

# Redacción de PII
[security.redaction]
replacement = "[REDACTED]"
partial_redaction = false
preserve_format = true

# Auditoría y logging
[security.audit]
enabled = true
level = "detailed"  # basic, detailed, forensic
retention_days = 90
encryption = true
hash_chain = true

[security.audit.events]
session_start = true
session_end = true
tool_invocation = true
provider_call = true
config_change = true
auth_event = true
error_event = true
```

### Control de Acceso
```toml
[security.access_control]
enabled = true
default_policy = "deny"  # deny, allow

# Reglas de acceso basadas en contexto
[security.access_control.rules]
# Regla 1: Solo herramientas de lectura en proyectos nuevos
- name = "new_project_readonly"
  condition = "project_age_days < 7"
  actions = ["file_read", "directory_tree", "grep", "glob"]
  permission = "allow"

# Regla 2: Sin herramientas destructivas en producción
- name = "no_destructive_in_prod"
  condition = "environment == 'production'"
  actions = ["bash", "file_write", "file_edit"]
  permission = "deny"

# Regla 3: Acceso completo en desarrollo
- name = "full_access_dev"
  condition = "environment == 'development'"
  actions = ["*"]
  permission = "allow"

# Regla 4: Horario restringido
- name = "business_hours_only"
  condition = "hour < 9 or hour > 18"
  actions = ["bash", "file_write"]
  permission = "deny"
```

### Cifrado y Almacenamiento Seguro
```toml
[security.encryption]
enabled = true
algorithm = "aes-256-gcm"
key_derivation = "argon2id"
key_storage = "system_keychain"  # system_keychain, encrypted_file

[security.encryption.data]
sessions = true
memory_entries = true
audit_logs = true
config_secrets = true

[security.key_management]
key_rotation_days = 90
backup_keys = true
emergency_access = false
```

## Configuración de Memoria

### Almacenamiento de Memoria
```toml
[memory]
enabled = true
storage_backend = "sqlite"  # sqlite, postgres, hybrid
max_entries = 10000
cleanup_interval_hours = 24

# Configuración SQLite
[memory.sqlite]
path = "~/.cuervo/memory.db"
journal_mode = "WAL"
synchronous = "NORMAL"
cache_size = -2000  # 2MB

# Configuración de tipos de memoria
[memory.types.fact]
enabled = true
max_entries = 1000
ttl_days = 30
quality_threshold = 0.7

[memory.types.code_snippet]
enabled = true
max_entries = 5000
ttl_days = 90
quality_threshold = 0.8

[memory.types.session_summary]
enabled = true
max_entries = 1000
ttl_days = 365

[memory.types.decision]
enabled = true
max_entries = 2000
ttl_days = 180
```

### Búsqueda y Recuperación
```toml
[memory.search]
enabled = true
engine = "hybrid"  # keyword, vector, hybrid
default_limit = 10
max_limit = 100

# Búsqueda por keyword (BM25)
[memory.search.keyword]
enabled = true
language = "spanish"  # spanish, english, portuguese
stop_words = true
stemming = true

# Búsqueda vectorial
[memory.search.vector]
enabled = true
model = "all-MiniLM-L6-v2"
dimensions = 384
similarity_threshold = 0.7

# Re-ranking
[memory.search.reranking]
enabled = true
model = "cross-encoder/ms-marco-MiniLM-L-6-v2"
top_k = 50
```

### Indexación y Mantenimiento
```toml
[memory.indexing]
enabled = true
auto_index = true
batch_size = 100
workers = 2

[memory.indexing.triggers]
on_session_end = true
on_tool_usage = true
on_code_change = true
interval_hours = 1

[memory.maintenance]
prune_interval_hours = 24
vacuum_interval_days = 7
backup_interval_days = 30
backup_retention_days = 90
```

## Configuración de Rendimiento

### Cache y Optimización
```toml
[performance]
enabled = true
cache_enabled = true
prefetch_enabled = true
compression_enabled = true

# Cache multi-nivel
[performance.cache]
memory_size_mb = 256
disk_size_mb = 1024
ttl_seconds = 3600

[performance.cache.levels]
l1_enabled = true
l1_size = 1000
l2_enabled = true
l2_size = 10000

# Cache de respuestas
[performance.cache.responses]
enabled = true
strategy = "content_hash"
ttl_seconds = 86400  # 24 horas
max_size_mb = 500

# Cache de embeddings
[performance.cache.embeddings]
enabled = true
ttl_seconds = 604800  # 7 días
max_entries = 10000
```

### Pool de Conexiones
```toml
[performance.connection_pool]
enabled = true

[performance.connection_pool.providers]
max_connections = 10
idle_timeout_secs = 300
connection_timeout_secs = 30

[performance.connection_pool.database]
max_connections = 5
idle_timeout_secs = 600
```

### Streaming y Procesamiento
```toml
[performance.streaming]
enabled = true
chunk_size_bytes = 4096
buffer_size_bytes = 16384
timeout_secs = 30

[performance.processing]
max_concurrent_tools = 3
max_concurrent_agents = 2
worker_threads = 4
```

## Configuración para Desarrollo

### Configuración de Debug
```toml
[development]
debug = true
trace_enabled = true
profile_enabled = false

[development.logging]
level = "trace"
format = "json"
file = "cuervo-debug.log"
max_file_size_mb = 100
max_files = 10

[development.tracing]
enabled = true
export_format = "json"
export_path = "./traces"
sampling_rate = 1.0  # 100%

[development.metrics]
enabled = true
export_interval_secs = 60
export_format = "prometheus"
```

### Testing y QA
```toml
[testing]
mock_providers = true
mock_tools = true
isolated_storage = true

[testing.providers]
echo_enabled = true
replay_enabled = true
timeout_secs = 5

[testing.tools]
sandbox_enabled = true
dry_run = true
confirm_prompts = false
```

### Configuración Local
```toml
[local]
data_dir = "./.cuervo-local"
config_dir = "./.cuervo-config"
cache_dir = "./.cuervo-cache"

[local.network]
proxy = null
proxy_auth = null
ssl_verify = true
```

## Configuración Enterprise

### Configuración Multi-Tenant
```toml
[enterprise]
multi_tenant = true
default_tenant = "default"
tenant_isolation = "strict"  # strict, relaxed, none

[enterprise.tenants.default]
name = "Default Tenant"
description = "Tenant por defecto"
quota_tokens_monthly = 1000000
quota_sessions_monthly = 1000
allowed_providers = ["anthropic", "openai", "ollama"]
allowed_tools = ["file_read", "directory_tree", "grep"]

[enterprise.tenants.research]
name = "Research Team"
description = "Equipo de investigación"
quota_tokens_monthly = 5000000
quota_sessions_monthly = 5000
allowed_providers = ["anthropic", "openai", "ollama", "gemini"]
allowed_tools = ["*"]  # Todas las herramientas
```

### SSO y Autenticación
```toml
[enterprise.auth]
enabled = true
provider = "oidc"  # oidc, saml, ldap
required = true

[enterprise.auth.oidc]
issuer = "https://auth.example.com"
client_id = "cuervo-cli"
client_secret_env = "OIDC_CLIENT_SECRET"
scopes = ["openid", "profile", "email"]

[enterprise.auth.saml]
metadata_url = "https://auth.example.com/saml/metadata"
entity_id = "cuervo-cli"
acs_url = "https://cuervo.example.com/saml/acs"
```

### Políticas y Compliance
```toml
[enterprise.policies]
data_retention_days = 365
audit_retention_days = 730
backup_retention_days = 90

[enterprise.policies.compliance]
gdpr_compliant = true
hipaa_compliant = false
soc2_compliant = true

[enterprise.policies.data_sovereignty]
region = "us-east-1"
encryption_at_rest = true
encryption_in_transit = true
```

### Monitoreo y Alertas
```toml
[enterprise.monitoring]
enabled = true
export_interval_secs = 30

[enterprise.monitoring.metrics]
provider = "prometheus"  # prometheus, datadog, newrelic
endpoint = "http://localhost:9090"
namespace = "cuervo"

[enterprise.monitoring.logging]
provider = "elastic"  # elastic, splunk, cloudwatch
endpoint = "http://localhost:9200"
index = "cuervo-logs"

[enterprise.monitoring.alerts]
enabled = true

[enterprise.monitoring.alerts.rules]
- name = "high_error_rate"
  condition = "error_rate > 0.05"
  severity = "warning"
  channels = ["slack", "email"]

- name = "quota_exceeded"
  condition = "tokens_used > quota * 0.9"
  severity = "critical"
  channels = ["pagerduty", "slack"]
```

---

## Variables de Entorno

### Variables Principales
```bash
# Configuración básica
export CUERVO_CONFIG="/path/to/config.toml"
export CUERVO_LOG="debug"
export CUERVO_NO_BANNER="1"

# Proveedores
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="AIza..."
export DEEPSEEK_API_KEY="sk-..."

# Configuración de red
export HTTP_PROXY="http://proxy.example.com:8080"
export HTTPS_PROXY="http://proxy.example.com:8080"
export NO_PROXY="localhost,127.0.0.1"

# Desarrollo
export RUST_LOG="cuervo=debug,tokio=info"
export RUST_BACKTRACE="1"
```

### Variables por Proveedor
```bash
# Anthropic
export ANTHROPIC_API_BASE="https://api.anthropic.com"
export ANTHROPIC_MAX_TOKENS="8192"
export ANTHROPIC_TIMEOUT="30"

# OpenAI
export OPENAI_API_BASE="https://api.openai.com/v1"
export OPENAI_ORG="org-xxx"

# Ollama
export OLLAMA_HOST="http://localhost:11434"
export OLLAMA_KEEP_ALIVE="5m"

# Gemini
export GEMINI_SAFETY_SETTINGS="moderate"
```

---

## Notas de Configuración

### Orden de Precedencia
1. **Argumentos de línea de comandos** (--model, --provider)
2. **Variables de entorno** (CUERVO_MODEL, CUERVO_PROVIDER)
3. **Configuración local** (`./.cuervo/config.toml`)
4. **Configuración global** (`~/.cuervo/config.toml`)
5. **Configuración por defecto** (`config/default.toml`)

### Migración de Configuración
```bash
# Exportar configuración actual
cuervo config show > current_config.toml

# Importar configuración
cp current_config.toml ~/.cuervo/config.toml

# Validar configuración
cuervo doctor --check-config
```

### Resolución de Problemas
```bash
# Verificar configuración cargada
cuervo config show --verbose

# Probar configuración específica
cuervo config get "models.providers.anthropic.enabled"

# Limpiar cache de configuración
rm -rf ~/.cuervo/cache/config.cache
```

---

*Última actualización: Febrero 2026*  
*Referencia: docs/08-enterprise-design/05-security-compliance-observability.md*

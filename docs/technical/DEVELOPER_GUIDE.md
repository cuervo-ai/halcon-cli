# Cuervo CLI - Documentación Técnica para Desarrolladores

## Tabla de Contenidos
1. [Arquitectura del Sistema](#arquitectura-del-sistema)
2. [Extensión del Sistema](#extensión-del-sistema)
3. [API Interna](#api-interna)
4. [Sistema de Herramientas](#sistema-de-herramientas)
5. [Sistema de Proveedores](#sistema-de-proveedores)
6. [Sistema de Memoria](#sistema-de-memoria)
7. [Sistema de Contexto](#sistema-de-contexto)
8. [Testing y Debugging](#testing-y-debugging)
9. [Performance y Optimización](#performance-y-optimización)
10. [Contribución al Código](#contribución-al-código)

## Arquitectura del Sistema

### Visión General
Cuervo CLI sigue una arquitectura modular basada en crates de Rust, diseñada para ser extensible, segura y de alto rendimiento.

### Componentes Principales

#### 1. **cuervo-core** - Núcleo del Sistema
```rust
// Estructuras fundamentales
pub mod traits {
    pub trait Provider {}      // Proveedores de IA
    pub trait Tool {}          // Herramientas del sistema
    pub trait Storage {}       // Almacenamiento persistente
    pub trait Context {}       // Gestión de contexto
}

pub mod types {
    pub struct Session {}      // Sesión de chat
    pub struct ChatMessage {}  // Mensaje de chat
    pub struct ToolCall {}     // Llamada a herramienta
    pub struct Config {}       // Configuración
}
```

#### 2. **cuervo-cli** - Interfaz de Usuario
- **REPL Interactivo**: Terminal interactiva con autocompletado
- **Sistema de Comandos**: CLI basado en clap con subcomandos
- **Renderizado**: Sistema de UI en terminal con temas y colores
- **Orquestador**: Coordinación de agentes y herramientas

#### 3. **cuervo-providers** - Integración con Modelos de IA
- **Interfaz unificada**: API común para todos los proveedores
- **Streaming**: Soporte para respuestas en tiempo real
- **Fallback**: Mecanismos de resiliencia entre proveedores
- **Caching**: Cache de respuestas para reducir costos

#### 4. **cuervo-tools** - Herramientas del Sistema
- **Sandboxing**: Ejecución segura de herramientas
- **Control de permisos**: Niveles granulares de acceso
- **Auditoría**: Registro completo de operaciones
- **Extensibilidad**: Fácil adición de nuevas herramientas

### Flujo de Datos
```
Usuario → CLI Parser → Config Loader → Provider Selector → Model Call
      ↓                                          ↓
Tool Registry ←─── Orchestrator ←─── Response Parser
      ↓                                          ↓
Tool Execution → Context Update → Memory Store → Response Render
```

## Extensión del Sistema

### Crear un Nuevo Proveedor

#### 1. Estructura del Crate
```bash
crates/
└── cuervo-provider-example/
    ├── src/
    │   ├── lib.rs
    │   ├── provider.rs
    │   └── types.rs
    └── Cargo.toml
```

#### 2. Implementación del Trait Provider
```rust
// crates/cuervo-provider-example/src/provider.rs
use async_trait::async_trait;
use cuervo_core::traits::Provider;
use cuervo_core::types::{ChatMessage, ChatResponse};

pub struct ExampleProvider {
    api_base: String,
    api_key: Option<String>,
}

#[async_trait]
impl Provider for ExampleProvider {
    async fn chat(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
        temperature: f32,
        max_tokens: Option<u32>,
    ) -> Result<ChatResponse, ProviderError> {
        // Implementación específica del proveedor
    }
    
    async fn stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
        temperature: f32,
        max_tokens: Option<u32>,
    ) -> Result<impl Stream<Item = Result<ChatChunk, ProviderError>>, ProviderError> {
        // Implementación de streaming
    }
}
```

#### 3. Registro en el Sistema
```rust
// crates/cuervo-providers/src/registry.rs
pub fn register_providers() -> HashMap<String, Box<dyn ProviderFactory>> {
    let mut registry = HashMap::new();
    
    // Registrar proveedor existente
    registry.insert("example".to_string(), Box::new(ExampleProviderFactory));
    
    registry
}
```

### Crear una Nueva Herramienta

#### 1. Implementación del Trait Tool
```rust
// crates/cuervo-tools/src/example_tool.rs
use async_trait::async_trait;
use cuervo_core::traits::Tool;
use serde_json::Value;

pub struct ExampleTool;

#[async_trait]
impl Tool for ExampleTool {
    fn name(&self) -> &'static str {
        "example_tool"
    }
    
    fn description(&self) -> &'static str {
        "Una herramienta de ejemplo para demostración"
    }
    
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "Entrada para procesar"
                }
            },
            "required": ["input"]
        })
    }
    
    async fn execute(&self, params: Value) -> Result<Value, ToolError> {
        let input = params["input"].as_str().unwrap();
        // Lógica de la herramienta
        Ok(json!({ "result": format!("Procesado: {}", input) }))
    }
    
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
}
```

#### 2. Registro de la Herramienta
```rust
// crates/cuervo-tools/src/registry.rs
pub fn register_tools() -> HashMap<String, Box<dyn Tool>> {
    let mut registry = HashMap::new();
    
    registry.insert("example_tool".to_string(), Box::new(ExampleTool));
    
    registry
}
```

## API Interna

### Sistema de Eventos
Cuervo CLI utiliza un sistema de eventos para desacoplar componentes:

```rust
// Definición de eventos
pub enum DomainEvent {
    SessionStarted { session_id: Uuid },
    ToolInvoked { tool_name: String, params: Value },
    ProviderCalled { provider: String, model: String },
    MemoryUpdated { entry_type: MemoryEntryType },
    ErrorOccurred { error: String, context: String },
}

// Uso del bus de eventos
let (tx, mut rx) = event_bus(100);

// Publicar evento
tx.send(DomainEvent::SessionStarted { session_id }).unwrap();

// Suscribirse a eventos
tokio::spawn(async move {
    while let Ok(event) = rx.recv().await {
        match event {
            DomainEvent::ToolInvoked { tool_name, params } => {
                // Procesar evento
            }
            _ => {}
        }
    }
});
```

### Sistema de Configuración

#### Jerarquía de Configuración
```rust
pub struct ConfigSystem {
    // Orden de precedencia (de mayor a menor):
    // 1. Flags de línea de comandos
    // 2. Variables de entorno
    // 3. Configuración local (.cuervo/config.toml)
    // 4. Configuración global (~/.cuervo/config.toml)
    // 5. Valores por defecto (config/default.toml)
    
    pub sources: Vec<ConfigSource>,
    pub cache: RwLock<HashMap<String, Value>>,
}

impl ConfigSystem {
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        // Resolución jerárquica
    }
    
    pub fn set(&self, key: &str, value: Value) -> Result<()> {
        // Actualización en la fuente apropiada
    }
}
```

### Sistema de Cache

#### Cache de Respuestas
```rust
pub struct ResponseCache {
    db: SqliteConnection,
    ttl: Duration,
}

impl ResponseCache {
    pub async fn get(
        &self,
        provider: &str,
        model: &str,
        messages: &[ChatMessage],
    ) -> Option<ChatResponse> {
        // Búsqueda por hash de la conversación
    }
    
    pub async fn set(
        &self,
        provider: &str,
        model: &str,
        messages: &[ChatMessage],
        response: &ChatResponse,
    ) -> Result<()> {
        // Almacenamiento con TTL
    }
}
```

## Sistema de Herramientas

### Arquitectura de Seguridad

#### Niveles de Permiso
```rust
pub enum PermissionLevel {
    ReadOnly,      // Solo lectura (file_read, directory_tree)
    ReadWrite,     // Lectura/escritura (file_edit, file_write)
    Destructive,   // Operaciones destructivas (bash con rm)
    System,        // Acceso a sistema (procesos, red)
}

pub struct ToolSecurity {
    pub level: PermissionLevel,
    pub allowed_dirs: Vec<PathBuf>,
    pub blocked_patterns: Vec<GlobPattern>,
    pub require_confirmation: bool,
}
```

#### Sandboxing de Herramientas
```rust
pub struct ToolSandbox {
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
    pub resource_limits: ResourceLimits,
    pub network_access: bool,
}

impl ToolSandbox {
    pub async fn execute(&self, tool: &dyn Tool, params: Value) -> Result<Value, ToolError> {
        // Ejecución en contexto aislado
        // - Chroot o namespaces en Linux
        // - Sandbox en macOS
        // - Job objects en Windows
    }
}
```

### Auditoría de Herramientas
```rust
pub struct ToolAudit {
    pub tool_name: String,
    pub params: Value,
    pub result: Result<Value, ToolError>,
    pub timestamp: DateTime<Utc>,
    pub user_id: Option<String>,
    pub session_id: Uuid,
    pub hash_chain: String,  // Para integridad de auditoría
}

pub struct AuditLogger {
    db: SqliteConnection,
    crypto_key: [u8; 32],
}

impl AuditLogger {
    pub fn log(&self, audit: ToolAudit) -> Result<()> {
        // Registro cifrado con hash chain
        // Verificación de integridad
        // Retención configurable
    }
}
```

## Sistema de Proveedores

### Patrón de Adaptador
```rust
pub trait ProviderAdapter {
    async fn normalize_request(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
    ) -> Result<ProviderRequest, ProviderError>;
    
    async fn normalize_response(
        &self,
        raw_response: ProviderRawResponse,
    ) -> Result<ChatResponse, ProviderError>;
    
    async fn handle_error(
        &self,
        error: reqwest::Error,
    ) -> ProviderError;
}

// Implementación para Anthropic
pub struct AnthropicAdapter {
    api_base: String,
    api_key: String,
}

impl ProviderAdapter for AnthropicAdapter {
    async fn normalize_request(
        &self,
        messages: Vec<ChatMessage>,
        model: &str,
    ) -> Result<ProviderRequest, ProviderError> {
        // Convertir mensajes de Cuervo a formato Anthropic
    }
}
```

### Sistema de Fallback
```rust
pub struct ProviderFallback {
    primary: String,
    fallbacks: Vec<String>,
    health_check: HealthChecker,
}

impl ProviderFallback {
    pub async fn get_provider(&self) -> Result<Box<dyn Provider>, ProviderError> {
        // Verificar salud del proveedor primario
        // Fallback automático si falla
        // Reintentos con backoff exponencial
    }
}
```

### Monitoreo de Proveedores
```rust
pub struct ProviderMetrics {
    latency: Histogram,
    success_rate: Gauge,
    token_usage: Counter,
    cost: Counter,
}

pub struct ProviderMonitor {
    metrics: HashMap<String, ProviderMetrics>,
    alerts: Vec<AlertRule>,
}

impl ProviderMonitor {
    pub fn record_call(
        &self,
        provider: &str,
        duration: Duration,
        success: bool,
        tokens: TokenUsage,
    ) {
        // Registro de métricas
        // Verificación de alertas
        // Reporte de salud
    }
}
```

## Sistema de Memoria

### Arquitectura de Almacenamiento
```rust
pub struct MemorySystem {
    // Capa 1: Cache en memoria (LRU)
    memory_cache: LruCache<String, MemoryEntry>,
    
    // Capa 2: SQLite (persistente)
    sqlite_store: SqliteMemoryStore,
    
    // Capa 3: Vector DB (búsqueda semántica)
    vector_store: Option<VectorMemoryStore>,
    
    // Capa 4: Cloud sync (opcional)
    cloud_sync: Option<CloudMemorySync>,
}

pub enum MemoryEntry {
    Fact {
        id: Uuid,
        content: String,
        source: MemorySource,
        confidence: f32,
        tags: Vec<String>,
        created_at: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    },
    CodeSnippet {
        id: Uuid,
        code: String,
        language: String,
        context: String,
        quality_score: f32,
        usage_count: u32,
    },
    // ... otros tipos
}
```

### Búsqueda Híbrida
```rust
pub struct HybridRetriever {
    keyword_retriever: KeywordRetriever,  // BM25
    vector_retriever: VectorRetriever,    // Embeddings
    reranker: CrossEncoderReranker,       // Re-ranking
}

impl HybridRetriever {
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        // Búsqueda por keyword
        let keyword_results = self.keyword_retriever.search(query, limit * 2).await?;
        
        // Búsqueda vectorial
        let vector_results = self.vector_retriever.search(query, limit * 2).await?;
        
        // Fusión de resultados
        let merged = self.merge_results(keyword_results, vector_results);
        
        // Re-ranking
        let reranked = self.reranker.rerank(query, merged, limit).await?;
        
        Ok(reranked)
    }
}
```

### Gestión del Ciclo de Vida
```rust
pub struct MemoryManager {
    store: Arc<dyn MemoryStore>,
    policies: MemoryPolicies,
}

impl MemoryManager {
    pub async fn prune(&self) -> Result<PruneStats, MemoryError> {
        // 1. Eliminar entradas expiradas
        let expired = self.store.delete_expired().await?;
        
        // 2. Eliminar entradas de baja calidad
        let low_quality = self.store.delete_low_quality(
            self.policies.min_quality_score
        ).await?;
        
        // 3. Aplicar límites por tipo
        let over_limit = self.store.enforce_limits(
            &self.policies.type_limits
        ).await?;
        
        Ok(PruneStats {
            expired,
            low_quality,
            over_limit,
        })
    }
}
```

## Sistema de Contexto

### Jerarquía de Contexto
```rust
pub struct ContextHierarchy {
    // Orden de resolución (de más específico a más general):
    // 1. Session Context (más específico)
    // 2. User Context
    // 3. Project Context
    // 4. Organization Context
    // 5. System Context (más general)
    
    layers: Vec<ContextLayer>,
    cache: ContextCache,
}

impl ContextHierarchy {
    pub async fn resolve<T: DeserializeOwned>(
        &self,
        key: &str,
        scope: ContextScope,
    ) -> Result<Option<T>, ContextError> {
        // Resolución en cascada a través de las capas
        for layer in self.layers.iter() {
            if layer.scope <= scope {
                if let Some(value) = layer.get(key).await? {
                    return Ok(Some(value));
                }
            }
        }
        Ok(None)
    }
}
```

### Contexto Semántico
```rust
pub struct SemanticContext {
    embedding_model: Arc<dyn EmbeddingModel>,
    vector_store: Arc<dyn VectorStore>,
    chunker: TextChunker,
}

impl SemanticContext {
    pub async fn index_project(&self, project_path: &Path) -> Result<(), ContextError> {
        // 1. Recorrer archivos del proyecto
        let files = self.scan_project(project_path).await?;
        
        // 2. Chunking inteligente
        let chunks = self.chunker.chunk_files(files).await?;
        
        // 3. Generar embeddings
        let embeddings = self.embedding_model.embed_batch(&chunks).await?;
        
        // 4. Almacenar en vector DB
        self.vector_store.upsert(chunks, embeddings).await?;
        
        Ok(())
    }
    
    pub async fn retrieve(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ContextChunk>, ContextError> {
        // Búsqueda semántica con RAG
        let query_embedding = self.embedding_model.embed(query).await?;
        let results = self.vector_store.search(query_embedding, limit).await?;
        
        Ok(results)
    }
}
```

## Testing y Debugging

### Testing E2E
```rust
// tests/cli_e2e.rs
#[test]
fn test_chat_basic() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = cuervo_cmd(&tmp);
    
    cmd.arg("chat")
        .arg("Hello, world!")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello"));
}

#[test]
fn test_tool_execution() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = cuervo_cmd(&tmp);
    
    cmd.arg("chat")
        .write_stdin("/tools list\n/exit")
        .assert()
        .success()
        .stdout(predicate::str::contains("Available tools"));
}
```

### Debugging con Trazas
```bash
# Habilitar trazas detalladas
CUERVO_LOG=debug cuervo chat

# Exportar trazas como JSON
cuervo trace export <session-id> > trace.json

# Reproducir sesión
cuervo replay <session-id> --verify

# Debug de proveedores
CUERVO_PROVIDER_DEBUG=1 cuervo chat
```

### Testing de Proveedores
```bash
# Ejecutar suite completa
./scripts/test_providers.sh

# Test específico
cargo test --test provider_e2e -- --nocapture

# Test de rendimiento
cargo bench --bench provider_benchmarks
```

## Performance y Optimización

### Optimizaciones Implementadas

#### 1. **Cache Multinivel**
```rust
pub struct MultiLevelCache {
    l1: LruCache<String, CachedValue>,      // Memoria (nanosegundos)
    l2: SqliteCache,                        // SQLite (microsegundos)
    l3: RedisCache,                         // Redis (milisegundos)
}

impl MultiLevelCache {
    pub async fn get(&self, key: &str) -> Option<CachedValue> {
        // Check L1 → L2 → L3
        // Promoción automática entre niveles
    }
}
```

#### 2. **Pool de Conexiones**
```rust
pub struct ConnectionPool<T> {
    factory: Arc<dyn Fn() -> T + Send + Sync>,
    pool: Vec<Arc<T>>,
    semaphore: Semaphore,
}

impl<T> ConnectionPool<T> {
    pub async fn get(&self) -> PooledConnection<T> {
        // Pool con límite de conexiones
        // Reutilización de conexiones
        // Health checking periódico
    }
}
```

#### 3. **Streaming Eficiente**
```rust
pub struct StreamingResponse {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>,
    buffer: BytesMut,
}

impl StreamingResponse {
    pub async fn process(&mut self) -> Result<String, Error> {
        while let Some(chunk) = self.stream.next().await {
            let chunk = chunk?;
            self.buffer.extend_from_slice(&chunk);
            
            // Procesamiento incremental
            if let Some(complete) = self.extract_complete_messages()? {
                return Ok(complete);
            }
        }
        Ok(String::from_utf8_lossy(&self.buffer).to_string())
    }
}
```

### Métricas de Performance
```rust
pub struct PerformanceMetrics {
    pub latency: HistogramVec,
    pub throughput: CounterVec,
    pub error_rate: GaugeVec,
    pub resource_usage: GaugeVec,
}

impl PerformanceMetrics {
    pub fn record_api_call(
        &self,
        provider: &str,
        duration: Duration,
        success: bool,
    ) {
        self.latency
            .with_label_values(&[provider])
            .observe(duration.as_secs_f64());
        
        self.throughput
            .with_label_values(&[provider])
            .inc();
        
        if !success {
            self.error_rate
                .with_label_values(&[provider])
                .inc();
        }
    }
}
```

## Contribución al Código

### Guías de Estilo

#### 1. **Código Rust**
```rust
// ✅ Correcto
pub async fn process_message(
    &self,
    message: ChatMessage,
    context: &Context,
) -> Result<ProcessedMessage, Error> {
    // ...
}

// ❌ Incorrecto
pub async fn process_message(&self,message:ChatMessage,context:&Context)->Result<ProcessedMessage,Error>{
    // ...
}
```

#### 2. **Documentación**
```rust
/// Procesa un mensaje de chat con el contexto dado.
///
/// # Arguments
/// * `message` - El mensaje a procesar
/// * `context` - Contexto actual de la sesión
///
/// # Returns
/// `Result<ProcessedMessage, Error>` - Mensaje procesado o error
///
/// # Examples
/// ```
/// let processed = processor.process_message(message, &context).await?;
/// ```
pub async fn process_message(
    &self,
    message: ChatMessage,
    context: &Context,
) -> Result<ProcessedMessage, Error> {
    // ...
}
```

#### 3. **Testing**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_process_message_success() {
        // Arrange
        let processor = MessageProcessor::new();
        let message = ChatMessage::user("Hello");
        let context = Context::default();
        
        // Act
        let result = processor.process_message(message, &context).await;
        
        // Assert
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_process_message_empty() {
        // Test de caso borde
    }
}
```

### Proceso de Desarrollo

#### 1. **Configuración del Entorno**
```bash
# Instalar herramientas de desarrollo
rustup component add clippy rustfmt

# Configurar pre-commit hooks
cp .githooks/pre-commit .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit

# Instalar dependencias de desarrollo
cargo install cargo-audit cargo-tarpaulin
```

#### 2. **Flujo de Trabajo**
```bash
# 1. Actualizar rama principal
git checkout main
git pull origin main

# 2. Crear rama de feature
git checkout -b feature/nueva-funcionalidad

# 3. Desarrollo y commits
git add .
git commit -m "feat: añadir nueva funcionalidad"

# 4. Ejecutar checks
cargo check
cargo test
cargo clippy -- -D warnings
cargo fmt --check

# 5. Push y crear PR
git push origin feature/nueva-funcionalidad
```

#### 3. **Review de Código**
- **Requisitos mínimos para PR**:
  - Todos los tests pasan
  - Cobertura de código mantenida o mejorada
  - Documentación actualizada
  - Sin warnings de clippy
  - Código formateado con rustfmt
  
- **Checklist de review**:
  - [ ] Funcionalidad correcta
  - [ ] Tests adecuados
  - [ ] Documentación clara
  - [ ] Performance aceptable
  - [ ] Seguridad considerada
  - [ ] Compatibilidad con versiones anteriores

### Troubleshooting Común

#### 1. **Problemas de Build**
```bash
# Limpiar cache de cargo
cargo clean

# Actualizar dependencias
cargo update

# Verificar toolchain
rustup show

# Build en modo debug para más información
RUST_BACKTRACE=1 cargo build
```

#### 2. **Problemas de Runtime**
```bash
# Habilitar logging detallado
RUST_LOG=debug cuervo chat

# Verificar configuración
cuervo config show

# Probar conectividad
cuervo doctor

# Reproducir con trazas
cuervo --trace-json chat 2> trace.json
```

#### 3. **Problemas de Dependencias**
```toml
# Cargo.toml - Especificar versiones exactas para debugging
[dependencies]
tokio = { version = "=1.35.1", features = ["full"] }
reqwest = { version = "=0.12.4", features = ["json"] }
```

---

## Recursos Adicionales

### Documentación
- [The Rust Programming Language](https://doc.rust-lang.org/book/)
- [Async Rust](https://rust-lang.github.io/async-book/)
- [Tokio Documentation](https://tokio.rs/)
- [SQLx Documentation](https://docs.rs/sqlx/)

### Herramientas de Desarrollo
- **rust-analyzer**: LSP para Rust
- **cargo-watch**: Rebuild automático
- **cargo-expand**: Expandir macros
- **cargo-udeps**: Dependencias no usadas

### Comunidad
- [Rust Users Forum](https://users.rust-lang.org/)
- [Rust Discord](https://discord.gg/rust-lang)
- [Cuervo CLI Discussions](https://github.com/cuervo-ai/cuervo-cli/discussions)

---

*Última actualización: Febrero 2026*  
*Mantenedores: Equipo de Desarrollo Cuervo CLI*

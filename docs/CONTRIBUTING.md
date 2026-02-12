# Guía de Contribución

¡Gracias por tu interés en contribuir a Cuervo CLI! Este documento proporciona guías y estándares para contribuir al proyecto.

## Tabla de Contenidos
1. [Código de Conducta](#código-de-conducta)
2. [¿Cómo Contribuir?](#cómo-contribuir)
3. [Configuración del Entorno](#configuración-del-entorno)
4. [Estructura del Proyecto](#estructura-del-proyecto)
5. [Estándares de Código](#estándares-de-código)
6. [Testing](#testing)
7. [Documentación](#documentación)
8. [Proceso de Pull Request](#proceso-de-pull-request)
9. [Reporte de Bugs](#reporte-de-bugs)
10. [Solicitud de Funcionalidades](#solicitud-de-funcionalidades)

## Código de Conducta

Este proyecto y todos los participantes se rigen por el [Código de Conducta](CODE_OF_CONDUCT.md). Al participar, se espera que mantengas este código. Por favor, reporta comportamientos inaceptables a [email del mantenedor].

## ¿Cómo Contribuir?

### Tipos de Contribuciones
1. **Reportar bugs**
2. **Sugerir mejoras**
3. **Contribuir código**
4. **Mejorar documentación**
5. **Traducciones**
6. **Tests**

### Primeros Pasos
1. Revisa los [issues existentes](https://github.com/cuervo-ai/cuervo-cli/issues)
2. Únete a las [discusiones](https://github.com/cuervo-ai/cuervo-cli/discussions)
3. Lee la [documentación](docs/)
4. Configura tu entorno de desarrollo

## Configuración del Entorno

### Requisitos
- **Rust 1.75+**: `rustup install stable`
- **SQLite 3.35+**: Generalmente incluido
- **Git**: Para control de versiones

### Configuración Inicial
```bash
# 1. Clonar el repositorio
git clone https://github.com/cuervo-ai/cuervo-cli
cd cuervo-cli

# 2. Configurar toolchain
rustup override set stable
rustup component add clippy rustfmt

# 3. Build inicial
cargo build

# 4. Instalar herramientas de desarrollo
cargo install cargo-audit cargo-tarpaulin cargo-watch
```

### Configuración del Editor
#### VS Code
```json
{
  "rust-analyzer.check.command": "clippy",
  "rust-analyzer.check.extraArgs": ["--", "-D", "warnings"],
  "editor.formatOnSave": true,
  "[rust]": {
    "editor.defaultFormatter": "rust-lang.rust-analyzer"
  }
}
```

#### IntelliJ/CLion
- Instalar plugin Rust
- Habilitar auto-import
- Configurar rustfmt en commit

## Estructura del Proyecto

```
cuervo-cli/
├── crates/                    # Workspace de Rust
│   ├── cuervo-cli/           # CLI principal
│   ├── cuervo-core/          # Tipos y traits core
│   ├── cuervo-providers/     # Proveedores de IA
│   ├── cuervo-tools/         # Herramientas del sistema
│   ├── cuervo-auth/          # Autenticación
│   ├── cuervo-storage/       # Almacenamiento
│   ├── cuervo-security/      # Seguridad
│   ├── cuervo-context/       # Sistema de contexto
│   └── cuervo-mcp/           # Integración MCP
├── docs/                     # Documentación
├── tests/                    # Tests de integración
├── scripts/                  # Scripts de utilidad
└── config/                   # Configuraciones
```

### Descripción de Crates

#### cuervo-core
- **Propósito**: Tipos y traits fundamentales
- **Dependencias**: serde, thiserror, async-trait
- **No debe depender de**: Otros crates del workspace

#### cuervo-providers
- **Propósito**: Integración con APIs de IA
- **Dependencias**: reqwest, serde_json, cuervo-core
- **Estructura**: Un módulo por proveedor

#### cuervo-tools
- **Propósito**: Herramientas del sistema
- **Dependencias**: tokio, glob, regex, cuervo-core
- **Características**: Sandboxing, auditoría

#### cuervo-cli
- **Propósito**: Interfaz de usuario
- **Dependencias**: clap, crossterm, reedline, todos los otros crates
- **Responsabilidades**: Parsing CLI, REPL, orquestación

## Estándares de Código

### Convenciones de Rust
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

### Documentación
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
/// let processor = MessageProcessor::new();
/// let processed = processor.process_message(message, &context).await?;
/// ```
///
/// # Errors
/// Retorna `Error::InvalidInput` si el mensaje está vacío.
pub async fn process_message(
    &self,
    message: ChatMessage,
    context: &Context,
) -> Result<ProcessedMessage, Error> {
    // ...
}
```

### Manejo de Errores
```rust
// ✅ Usar thiserror para errores de dominio
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("API request failed: {0}")]
    ApiError(#[from] reqwest::Error),
    
    #[error("Invalid API key")]
    InvalidApiKey,
    
    #[error("Rate limited, retry after {0} seconds")]
    RateLimited(u64),
}

// ✅ Usar anyhow para errores de aplicación
use anyhow::{Context, Result};

pub async fn load_config(path: &Path) -> Result<Config> {
    let content = fs::read_to_string(path)
        .context(format!("Failed to read config from {}", path.display()))?;
    
    toml::from_str(&content)
        .context("Failed to parse config TOML")
}
```

### Patrones Comunes

#### Builder Pattern
```rust
pub struct ChatRequestBuilder {
    messages: Vec<ChatMessage>,
    model: String,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
}

impl ChatRequestBuilder {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            messages: Vec::new(),
            model: model.into(),
            temperature: None,
            max_tokens: None,
        }
    }
    
    pub fn add_message(mut self, message: ChatMessage) -> Self {
        self.messages.push(message);
        self
    }
    
    pub fn build(self) -> ChatRequest {
        ChatRequest {
            messages: self.messages,
            model: self.model,
            temperature: self.temperature.unwrap_or(0.0),
            max_tokens: self.max_tokens,
        }
    }
}
```

#### Factory Pattern
```rust
pub trait ProviderFactory: Send + Sync {
    fn create(&self, config: &ProviderConfig) -> Result<Box<dyn Provider>>;
    fn supported_models(&self) -> Vec<String>;
}

pub struct AnthropicFactory;

impl ProviderFactory for AnthropicFactory {
    fn create(&self, config: &ProviderConfig) -> Result<Box<dyn Provider>> {
        Ok(Box::new(AnthropicProvider::new(
            config.api_base.clone(),
            config.api_key.clone(),
        )?))
    }
    
    fn supported_models(&self) -> Vec<String> {
        vec![
            "claude-3-5-sonnet-20241022".into(),
            "claude-3-opus-20240229".into(),
            "claude-3-haiku-20240307".into(),
        ]
    }
}
```

## Testing

### Tipos de Tests
```bash
# Tests unitarios
cargo test --lib

# Tests de integración
cargo test --test '*'

# Tests E2E
cargo test --test cli_e2e

# Tests de performance
cargo bench

# Tests de seguridad
cargo audit
```

### Estructura de Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    // Tests unitarios
    #[test]
    fn test_basic_functionality() {
        // Arrange
        let input = "test";
        
        // Act
        let result = process(input);
        
        // Assert
        assert_eq!(result, "processed test");
    }
    
    // Tests async
    #[tokio::test]
    async fn test_async_function() {
        let result = async_function().await;
        assert!(result.is_ok());
    }
    
    // Tests con fixtures
    #[test]
    fn test_with_fixture() {
        let tmp = TempDir::new().unwrap();
        // Usar tmp para archivos temporales
    }
}
```

### Mocking
```rust
// tests/mocks.rs
pub struct MockProvider {
    responses: Vec<ChatResponse>,
}

#[async_trait]
impl Provider for MockProvider {
    async fn chat(&self, _: Vec<ChatMessage>, _: &str) -> Result<ChatResponse> {
        Ok(self.responses.pop().unwrap())
    }
}

// tests/integration.rs
#[test]
fn test_with_mock() {
    let mock = MockProvider::new();
    let processor = Processor::new(Box::new(mock));
    // Test con mock
}
```

### Coverage
```bash
# Generar reporte de coverage
cargo tarpaulin --ignore-tests --out Html

# Coverage mínimo requerido: 80%
# Objetivo: 90%+
```

## Documentación

### Documentación de Código
- **Todo código público debe estar documentado**
- **Ejemplos en documentación deben compilar**
- **Documentar errores posibles**
- **Usar markdown en doc comments**

### Documentación de Features
```markdown
# Nombre de la Feature

## Descripción
Breve descripción de la funcionalidad.

## Motivación
Por qué se necesita esta feature.

## Diseño Técnico
- Arquitectura
- Flujo de datos
- Consideraciones de performance

## API
```rust
// Ejemplos de código
```

## Configuración
```toml
# Ejemplos de configuración
```

## Testing
Cómo probar la feature.

## Consideraciones de Seguridad
Impacto en seguridad.

## Compatibilidad
Breaking changes, migración.
```

### Traducciones
- **Documentación principal en español**
- **Traducciones al inglés bienvenidas**
- **Mantener consistencia de términos**
- **Usar variables para textos repetidos**

## Proceso de Pull Request

### 1. Antes de Empezar
- [ ] Revisar issues existentes
- [ ] Discutir en GitHub Discussions si es necesario
- [ ] Asegurarse de entender la arquitectura

### 2. Crear una Rama
```bash
# Desde main actualizada
git checkout main
git pull origin main

# Crear rama descriptiva
git checkout -b feature/nombre-descriptivo
# o
git checkout -b fix/descripcion-del-fix
# o
git checkout -b docs/mejora-documentacion
```

### 3. Desarrollar
```bash
# Hacer commits pequeños y descriptivos
git add .
git commit -m "feat: añadir nueva funcionalidad"
git commit -m "fix: corregir bug en procesamiento"
git commit -m "docs: actualizar guía de instalación"

# Mantener la rama actualizada
git fetch origin
git rebase origin/main
```

### 4. Checks Locales
```bash
# Formatear código
cargo fmt

# Linting
cargo clippy -- -D warnings

# Tests
cargo test

# Build
cargo build --release

# Security audit
cargo audit
```

### 5. Crear Pull Request
1. **Título descriptivo**: "feat: añadir soporte para proveedor X"
2. **Descripción detallada**:
   - Qué cambia
   - Por qué es necesario
   - Cómo probarlo
   - Screenshots si aplica
3. **Referenciar issues**: "Closes #123"
4. **Seleccionar reviewers**
5. **Asignar labels**

### 6. Review Process
- **Dos approvals requeridos**
- **Todos los checks deben pasar**
- **Resolucionar comentarios**
- **Mantener discusión civilizada**

### 7. Merge
- **Squash commits** (generalmente)
- **Delete branch después de merge**
- **Actualizar CHANGELOG.md**
- **Actualizar documentación si es necesario**

## Reporte de Bugs

### Plantilla de Bug Report
```markdown
## Descripción
Descripción clara y concisa del bug.

## Pasos para Reproducir
1. Ir a '...'
2. Hacer click en '....'
3. Scroll hasta '....'
4. Ver error

## Comportamiento Esperado
Descripción de lo que debería pasar.

## Comportamiento Actual
Descripción de lo que pasa actualmente.

## Screenshots
Si aplica, añadir screenshots.

## Contexto Adicional
- Versión de Cuervo CLI:
- Sistema Operativo:
- Versión de Rust:
- Proveedor configurado:
- Configuración relevante:

## Logs
```
Pegar logs relevantes aquí
```

## Posible Solución
Si tienes ideas de cómo solucionarlo.
```

### Severidad de Bugs
- **Critical**: Crash, data loss, security vulnerability
- **High**: Feature broken, incorrect behavior
- **Medium**: Minor issue, workaround exists
- **Low**: Cosmetic, typo, documentation

## Solicitud de Funcionalidades

### Plantilla de Feature Request
```markdown
## Problema
Descripción del problema que esta feature resolvería.

## Solución Propuesta
Descripción de la solución.

## Alternativas Consideradas
Otras soluciones posibles.

## Beneficios
Por qué esta feature es valiosa.

## Impacto
- Usuarios afectados
- Cambios en API
- Performance
- Seguridad

## Ejemplos de Uso
```rust
// Código de ejemplo
```

## Consideraciones Técnicas
- Dependencias nuevas
- Compatibilidad
- Testing

## Prioridad
- [ ] Alta (blocker)
- [ ] Media (importante)
- [ ] Baja (nice to have)
```

### Criterios de Aceptación
- **Alta demanda**: Múltiples usuarios piden lo mismo
- **Alineación con visión**: Coherente con la dirección del producto
- **Factibilidad técnica**: Posible de implementar
- **Mantenibilidad**: No añade complejidad excesiva

## Reconocimiento

### Contribuidores
Los contribuidores serán reconocidos en:
- **CHANGELOG.md**
- **README.md**
- **GitHub Contributors**
- **Documentación**

### Niveles de Contribución
- **Contribuidor**: Una contribución aceptada
- **Collaborator**: Múltiples contribuciones significativas
- **Maintainer**: Responsabilidad sobre área específica
- **Core Team**: Decisión sobre dirección del proyecto

## Preguntas Frecuentes

### ¿Dónde pedir ayuda?
- **GitHub Discussions**: Para preguntas generales
- **GitHub Issues**: Para bugs y features
- **Documentación**: Para guías y tutoriales

### ¿Cómo empezar con contribuciones pequeñas?
1. Corregir typos en documentación
2. Mejorar mensajes de error
3. Añadir tests
4. Actualizar dependencias

### ¿Qué hacer si mi PR está estancado?
- @mention a reviewers después de 3 días
- Preguntar en Discussions
- Ofrecer hacer cambios solicitados

### ¿Cómo manejar breaking changes?
- Discutir ampliamente antes de implementar
- Proveer migración path
- Avisar con antelación en CHANGELOG
- Considerar feature flags

---

## Contacto

- **Issues**: [GitHub Issues](https://github.com/cuervo-ai/cuervo-cli/issues)
- **Discussions**: [GitHub Discussions](https://github.com/cuervo-ai/cuervo-cli/discussions)
- **Documentación**: [docs/](docs/)
- **Email**: [email del mantenedor]

---

*Última actualización: Febrero 2026*  
*Mantenedores: Equipo de Desarrollo Cuervo CLI*
